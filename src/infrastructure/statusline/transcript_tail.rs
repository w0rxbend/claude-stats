//! Reading the last billable turn out of a transcript without reading the
//! whole file.
//!
//! A transcript is JSON Lines, and the line this module wants is normally
//! within a few kilobytes of the end -- the statusline hook only ever asks
//! for the *current* turn's usage, and Claude Code appends to the file as a
//! session runs. Reading forward from the start would mean loading a
//! transcript that can run to tens of megabytes over a long session, on every
//! prompt redraw the cache does not absorb; reading backward in chunks means
//! the common case touches only the tail.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};

use crate::application::ports::TranscriptTailReader;
use crate::domain::tokens::TokenUsage;
use crate::infrastructure::transcript::records::Record;

/// How much of the file is read at a time, working backward from the end.
///
/// Sixty-four kilobytes comfortably holds several turns' worth of lines --
/// this crate's own fixtures run a few hundred bytes a line -- so the common
/// case answers from the first chunk read, and a transcript with an unusually
/// verbose tail simply costs a second chunk rather than a rewrite.
const CHUNK_BYTES: u64 = 64 * 1024;

/// Reads a transcript straight off disk.
#[derive(Debug, Default, Clone, Copy)]
pub struct FileSystemTranscriptTail;

impl TranscriptTailReader for FileSystemTranscriptTail {
    fn last_turn_usage(&self, path: &Path) -> Result<Option<TokenUsage>> {
        last_turn_usage(path)
    }
}

/// The free function [`FileSystemTranscriptTail::last_turn_usage`] delegates
/// to, kept outside the trait impl so it can be called directly from this
/// module's own tests without a fake to satisfy the port.
fn last_turn_usage(path: &Path) -> Result<Option<TokenUsage>> {
    let mut file =
        File::open(path).with_context(|| format!("cannot open transcript {}", path.display()))?;
    let len = file
        .metadata()
        .with_context(|| format!("cannot inspect transcript {}", path.display()))?
        .len();
    if len == 0 {
        return Ok(None);
    }

    let mut position = len;
    // Bytes read so far that have not yet been confirmed as a complete line:
    // the partial line hanging off the front of what has been read, which the
    // next, earlier chunk will complete.
    let mut pending: Vec<u8> = Vec::new();
    let mut buffer = vec![0u8; CHUNK_BYTES.min(len) as usize];

    loop {
        let read_len = CHUNK_BYTES.min(position);
        position -= read_len;
        file.seek(SeekFrom::Start(position))?;
        let slice = &mut buffer[..read_len as usize];
        file.read_exact(slice)?;

        let mut combined = Vec::with_capacity(slice.len() + pending.len());
        combined.extend_from_slice(slice);
        combined.extend_from_slice(&pending);

        // Peel complete lines off the back of `combined`, newest first, which
        // is exactly the order this function wants to search in. Everything
        // before the last newline still found is carried into the next,
        // earlier chunk as the new `pending`.
        let mut search_end = combined.len();
        while let Some(newline) = combined[..search_end].iter().rposition(|&b| b == b'\n') {
            if let Some(usage) = usage_of_line(&combined[newline + 1..search_end]) {
                return Ok(Some(usage));
            }
            search_end = newline;
        }
        pending = combined[..search_end].to_vec();

        if position == 0 {
            // Nothing precedes what is left in `pending`, so it is the
            // file's first line -- never peeled off above because no
            // newline comes before it.
            return Ok(usage_of_line(&pending));
        }
    }
}

/// The usage of `line`, if it is a well-formed `assistant` line that carries
/// one.
///
/// Anything else -- a line from another turn, a half-written line at a chunk
/// boundary, a line that fails to parse at all -- answers `None` rather than
/// an error. A transcript is appended to live, so the tail is routinely
/// mid-write, and the same tolerance every other reader in this crate has for
/// a malformed final line applies here too.
fn usage_of_line(line: &[u8]) -> Option<TokenUsage> {
    let record: Record = serde_json::from_slice(line).ok()?;
    if record.r#type != "assistant" {
        return None;
    }
    Some(record.message?.usage?.into())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// A file that deletes itself when the test ends.
    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn holding(name: &str, contents: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "claude-stats-tail-{name}-{}-{:?}.jsonl",
                std::process::id(),
                std::thread::current().id()
            ));
            let mut file = File::create(&path).expect("a writable temp file");
            file.write_all(contents.as_bytes()).expect("write");
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn line(kind: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"type":"{kind}","message":{{"usage":{{"input_tokens":{input},"output_tokens":{output}}}}}}}"#
        )
    }

    #[test]
    fn the_last_assistant_line_with_usage_wins_over_earlier_ones() {
        let contents = format!(
            "{}\n{}\n{}\n",
            line("assistant", 100, 10),
            line("user", 0, 0),
            line("assistant", 250, 25),
        );
        let file = TempFile::holding("last-wins", &contents);

        let usage = last_turn_usage(&file.0)
            .expect("a readable file")
            .expect("an assistant line with usage");
        assert_eq!(usage.input, 250);
        assert_eq!(usage.output, 25);
    }

    #[test]
    fn a_file_with_no_trailing_newline_is_still_read_correctly() {
        let contents = line("assistant", 42, 7);
        let file = TempFile::holding("no-trailing-newline", &contents);

        let usage = last_turn_usage(&file.0)
            .expect("a readable file")
            .expect("the only line, which is the assistant's");
        assert_eq!(usage.input, 42);
    }

    #[test]
    fn a_line_wider_than_one_chunk_is_still_found() {
        // A single content block padded well past `CHUNK_BYTES`, so the
        // backward scan must cross a chunk boundary in the middle of the very
        // line it is looking for.
        let padding = "x".repeat(CHUNK_BYTES as usize * 2);
        let contents = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{padding}"}}],"usage":{{"input_tokens":9,"output_tokens":1}}}}}}"#
        );
        let file = TempFile::holding("wide-line", &contents);

        let usage = last_turn_usage(&file.0)
            .expect("a readable file")
            .expect("the assistant line, however wide");
        assert_eq!(usage.input, 9);
    }

    #[test]
    fn an_empty_transcript_has_no_usage_to_report() {
        let file = TempFile::holding("empty", "");
        assert_eq!(last_turn_usage(&file.0).expect("a readable file"), None);
    }

    #[test]
    fn a_transcript_with_no_assistant_line_has_no_usage_to_report() {
        let contents = format!("{}\n", line("user", 0, 0));
        let file = TempFile::holding("no-assistant", &contents);
        assert_eq!(last_turn_usage(&file.0).expect("a readable file"), None);
    }

    #[test]
    fn a_missing_file_is_an_error_rather_than_a_silent_none() {
        let missing = std::env::temp_dir().join("claude-stats-tail-does-not-exist.jsonl");
        assert!(last_turn_usage(&missing).is_err());
    }
}
