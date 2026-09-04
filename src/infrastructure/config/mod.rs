//! The user's own presentation choices, read from an optional file.
//!
//! Structurally this is [`crate::infrastructure::pricing::overrides::PriceOverrides`]'s
//! twin -- a Gateway (Fowler, *`PoEAA`*) that owns every detail of talking to
//! one file under `${XDG_CONFIG_HOME:-~/.config}/claude-stats/`, so that
//! nothing above it (`main.rs`'s composition root, `App`) knows the resource
//! is a file at all. But the two gateways deliberately part ways on error
//! handling, and that divergence is the whole point of this module existing
//! separately rather than the price overrides growing a second file: a
//! malformed `prices.json` is refused outright, because somebody sat down
//! and corrected a rate on purpose and a shrug would leave them reading
//! figures they believe are fixed. A malformed `config.json` is a *cosmetic*
//! problem -- a wrong theme name, a typo in a keymap override -- and refusing
//! to start the dashboard over one would be a strictly worse outcome than
//! falling back to the defaults the user had before they ever wrote the
//! file. [`ConfigGateway::load_or_default`] therefore never returns an
//! `Err`; the worst a broken file can do is print one line to `stderr` and
//! leave a short note in the footer.
//!
//! # The file
//!
//! `${XDG_CONFIG_HOME:-~/.config}/claude-stats/config.json`, shaped like
//! this:
//!
//! ```json
//! {
//!   "theme": "aurora",
//!   "layout": "live",
//!   "animation": "pulse",
//!   "keymap": { "bind": [ { "keys": "gt", "action": "next-view" } ] },
//!   "layouts": {
//!     "my-layout": { "type": "panel", "panel": "panel.spend-panel" }
//!   }
//! }
//! ```
//!
//! Every field is optional, and a file that sets none of them is exactly
//! [`Config::default`] -- the same "present but empty is not a problem"
//! stance [`crate::infrastructure::pricing::overrides`] takes towards an
//! override file that lists no models.
//!
//! # Two different kinds of "wrong"
//!
//! A name like `"theme": "aurroa"` and a string that is not JSON at all are
//! both failures, but they are caught at two different points in this
//! module, on purpose:
//!
//! * **Malformed JSON** is caught inside [`ConfigGateway::load_or_default`]
//!   itself, because deserialising the file is the one step nothing else
//!   here can do instead.
//! * **A theme, layout or panel name JSON parses fine but does not resolve
//!   to anything real** is caught by the free function [`resolve`], which
//!   takes the [`Config`] `load_or_default` already produced and checks it
//!   against the actual [`crate::tui::palette::registry::ThemeRegistry`] and
//!   the actual layout presets. Splitting the two apart like this is what
//!   lets `load_or_default` stay a method on `ConfigGateway` with no registry
//!   to thread through it, while `resolve` stays a pure function a test can
//!   call with a `Config` it built by hand, without a file on disk anywhere.
//!   `src/main.rs`'s composition root is the one place that calls both, in
//!   order, and is the only caller that ever needs to.
//!
//! # Why this Gateway imports `crate::tui`
//!
//! Every other Gateway in `src/infrastructure` is written against
//! `crate::domain` types and nothing above them, because a business rule
//! must not know a terminal exists. This one is the deliberate exception:
//! its entire subject matter -- which theme, which layout, which animation,
//! which key bindings -- *is* presentation, so [`Config`] mirrors
//! [`crate::tui::widgets::dollar_pulse::AnimationStyle`] and
//! [`crate::tui::layout::Node`] directly rather than inventing a
//! domain-shaped echo of them that `src/tui` would then have to translate
//! back. The dependency still only points one way -- `crate::tui` never
//! imports anything from this module back -- so the hexagon's outward-facing
//! rule (nothing *inside* `domain` or `application` ever imports `tui` or
//! this module) is intact; this Gateway simply sits in `infrastructure`
//! because reading a file is what it does, not because what it reads is
//! domain data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub mod keymap_file;

/// The whole config file, or the defaults a dashboard starts with when the
/// user has never written one.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// A [`crate::tui::palette::Palette`] name from
    /// [`crate::tui::palette::registry::ThemeRegistry`], e.g. `"aurora"`.
    /// `None` (the default) means "use the crate's own default theme".
    pub theme: Option<String>,
    /// A layout preset name -- one of `crate::tui::layout::presets`'s four,
    /// or a key of [`Config::layouts`]. `None` means "use the crate's own
    /// default layout".
    pub layout: Option<String>,
    /// How `panel.dollar-pulse` animates. Already `Serialize`/`Deserialize`
    /// as of the epic that introduced it, so nothing further is needed here
    /// beyond naming the field.
    #[serde(default)]
    pub animation: crate::tui::widgets::dollar_pulse::AnimationStyle,
    /// Key bindings the user has added on top of
    /// [`crate::tui::keymap::Keymap::default_bindings`].
    #[serde(default)]
    pub keymap: KeymapOverrides,
    /// Named layout trees the user has written by hand, addressable from
    /// [`Config::layout`] alongside the four built-in preset names.
    #[serde(default)]
    pub layouts: HashMap<String, LayoutNodeDto>,
}

/// The `"keymap"` section of the config file: bindings the user has added.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct KeymapOverrides {
    pub bind: Vec<BindingDto>,
}

/// One user-authored key binding, as written in the file -- e.g.
/// `{ "keys": "gt", "action": "next-view" }`. `keys` is parsed by
/// [`keymap_file::parse_key_spec`]; `action` is left as a plain string here
/// because turning it into a [`crate::tui::keymap::NormalAction`], and
/// actually applying it to the live [`crate::tui::keymap::Keymap`], is the
/// next epic's job -- this one only has to prove the file loads and
/// degrades safely.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct BindingDto {
    pub keys: String,
    pub action: String,
}

/// A serde-friendly mirror of [`crate::tui::layout::Node`]/[`crate::tui::layout::SizeHint`].
///
/// That module has no serde derives, by design: it is a pure
/// rendering-adjacent type used by the hard-coded presets too, and giving it
/// a `#[serde(tag = "type")]` shape purely so a config file can be read would
/// tie its representation to JSON forever. This `Dto` (Fowler, *`PoEAA`*'s
/// Data Transfer Object) carries exactly the same tree shape across the
/// serde boundary and converts to the real `Node` with [`TryFrom`], which is
/// where a config-file typo -- an unknown panel id, an axis that is neither
/// `"row"` nor `"column"` -- is caught and turned into a message rather than
/// a panic deep inside the layout solver.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum LayoutNodeDto {
    Panel {
        panel: String,
    },
    Split {
        axis: String,
        children: Vec<LayoutChildDto>,
    },
}

/// One child of a [`LayoutNodeDto::Split`]: how much of the split it asks
/// for, and what it is.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct LayoutChildDto {
    pub size: SizeDto,
    pub node: LayoutNodeDto,
}

/// A serde-friendly mirror of [`crate::tui::layout::SizeHint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeDto {
    Fixed(u16),
    Weight(u16),
    Min(u16),
}

impl From<SizeDto> for crate::tui::layout::SizeHint {
    fn from(dto: SizeDto) -> Self {
        match dto {
            SizeDto::Fixed(n) => Self::Fixed(n),
            SizeDto::Weight(n) => Self::Weight(n),
            SizeDto::Min(n) => Self::Min(n),
        }
    }
}

impl TryFrom<LayoutNodeDto> for crate::tui::layout::Node {
    type Error = anyhow::Error;

    fn try_from(dto: LayoutNodeDto) -> Result<Self, Self::Error> {
        use crate::tui::layout::{Axis, Node};

        match dto {
            LayoutNodeDto::Panel { panel } => Ok(Node::Panel {
                id: resolve_panel_id(&panel)?,
            }),
            LayoutNodeDto::Split { axis, children } => {
                let axis = match axis.as_str() {
                    "row" => Axis::Row,
                    "column" => Axis::Column,
                    other => anyhow::bail!(
                        "{other:?} is not a layout axis (expected \"row\" or \"column\")"
                    ),
                };
                let children = children
                    .into_iter()
                    .map(TryFrom::try_from)
                    .collect::<anyhow::Result<Vec<_>>>()?;
                Ok(Node::Split { axis, children })
            }
        }
    }
}

impl TryFrom<LayoutChildDto> for crate::tui::layout::Child {
    type Error = anyhow::Error;

    fn try_from(dto: LayoutChildDto) -> Result<Self, Self::Error> {
        Ok(Self {
            node: dto.node.try_into()?,
            size: dto.size.into(),
        })
    }
}

/// Turns a config-file panel-id string into the crate's own
/// [`crate::tui::layout::PanelId`], checking it against
/// [`crate::tui::panels::PanelRegistry::builtin`] on the way.
///
/// `PanelId` wraps a `&'static str` -- see its own doc comment for why --
/// which a string freshly read out of a config file can never naturally be.
/// Leaking it with [`Box::leak`] is the ordinary, safe way to mint one
/// (`unsafe` is forbidden crate-wide, and this needs none): this runs at
/// most once per distinct panel id a hand-authored config file names, so the
/// handful of bytes it costs for the remaining life of the process is not
/// worth giving `PanelId` an owned-`String` variant just to spare it.
fn resolve_panel_id(name: &str) -> anyhow::Result<crate::tui::layout::PanelId> {
    let id = crate::tui::layout::PanelId(Box::leak(name.to_owned().into_boxed_str()));
    if crate::tui::panels::PanelRegistry::builtin()
        .get(&id)
        .is_some()
    {
        Ok(id)
    } else {
        anyhow::bail!("{name:?} is not a registered panel")
    }
}

/// A short, user-facing note about something in the config file that could
/// not be honoured, meant for the dashboard's own footer notice slot --
/// see `crate::tui::app::App`'s `config_warning` field -- rather than for a
/// log a user is unlikely to ever read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub message: String,
}

/// The user's presentation config, wherever they keep it.
#[derive(Debug, Clone)]
pub struct ConfigGateway {
    path: PathBuf,
}

/// The directory the file lives in, under whichever config root applies.
/// Matches [`crate::infrastructure::pricing::overrides::PriceOverrides`]'s
/// own `DIRECTORY` constant exactly, since the two files are meant to sit
/// side by side.
const DIRECTORY: &str = "claude-stats";
/// The file itself.
const FILE: &str = "config.json";

impl ConfigGateway {
    /// Points at `${XDG_CONFIG_HOME:-~/.config}/claude-stats/config.json`.
    ///
    /// Resolved the same way
    /// [`PriceOverrides::from_config_dir`](crate::infrastructure::pricing::overrides::PriceOverrides::from_config_dir)
    /// resolves its own path: a manual `XDG_CONFIG_HOME` check, falling back
    /// to [`dirs::home_dir`] joined with `.config` rather than
    /// [`dirs::config_dir`], so `config.json` always sits beside
    /// `prices.json` on both Linux and macOS regardless of which platform
    /// convention `dirs::config_dir` would otherwise have picked.
    ///
    /// # Errors
    ///
    /// Returns an error only when `XDG_CONFIG_HOME` is unset *and* the home
    /// directory cannot be determined, which on a normal machine means
    /// something is badly wrong with the environment -- the same narrow
    /// failure mode `PriceOverrides::from_config_dir` has.
    pub fn from_config_dir() -> anyhow::Result<Self> {
        let config_dir = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => dirs::home_dir()
                .context("cannot determine the home directory")?
                .join(".config"),
        };
        Ok(Self::at(config_dir.join(DIRECTORY).join(FILE)))
    }

    /// Points at an arbitrary file. Used by the tests, and by
    /// [`ConfigGateway::from_config_dir`] itself.
    #[must_use]
    pub const fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where this gateway is looking.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The user's config, or [`Config::default`] if there is none to read or
    /// none that parses.
    ///
    /// This never returns an `Err` -- see the module documentation for why a
    /// cosmetic file behaves nothing like the price overrides' own
    /// all-or-nothing refusal. Three cases, in order:
    ///
    /// 1. **No file at all** (the overwhelmingly common case, matching
    ///    [`PriceOverrides::load`](crate::infrastructure::pricing::overrides::PriceOverrides::load)'s
    ///    own missing-file-is-`Ok(None)` contract): `(Config::default(),
    ///    None)`, with nothing printed anywhere. Almost nobody will ever
    ///    write this file, and treating its absence as noteworthy would make
    ///    the normal case look like a problem.
    /// 2. **A file that exists but cannot even be read** (permissions, a
    ///    directory where a file was expected, and so on): the defaults,
    ///    plus a warning, since this is a real problem a user configured
    ///    their way into and silence would hide it just as badly as a parse
    ///    failure would.
    /// 3. **A file that exists and reads but does not parse as [`Config`]**:
    ///    one line to `stderr` naming the file and the exact line and
    ///    column `serde_json` reports, printed here so it reaches a real
    ///    terminal before `main.rs`'s composition root ever calls
    ///    `ratatui::try_init` and switches to the alternate screen -- and the
    ///    same fact, rephrased as one short sentence, returned as a
    ///    [`ConfigWarning`] for the footer, which is the only place left to
    ///    say it once the alternate screen is up.
    ///
    /// Checking a theme, layout or panel name the file *does* parse into
    /// against the real registries is deliberately not this method's job --
    /// see [`resolve`].
    #[must_use]
    pub fn load_or_default(&self) -> (Config, Option<ConfigWarning>) {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Config::default(), None);
            }
            Err(error) => {
                eprintln!("claude-stats: {}: {error}", self.path.display());
                return (
                    Config::default(),
                    Some(ConfigWarning {
                        message: format!("could not read {}: {error}", self.path.display()),
                    }),
                );
            }
        };

        match serde_json::from_str::<Config>(&text) {
            Ok(config) => (config, None),
            Err(error) => {
                eprintln!(
                    "claude-stats: {}: {} (line {}, column {})",
                    self.path.display(),
                    error,
                    error.line(),
                    error.column()
                );
                (
                    Config::default(),
                    Some(ConfigWarning {
                        message: format!(
                            "{} is not valid config (line {}); using defaults",
                            self.path.display(),
                            error.line()
                        ),
                    }),
                )
            }
        }
    }

    /// Read, apply `mutate`, write the whole document back.
    ///
    /// This is a read-merge-write, not a blind overwrite: it starts from
    /// whatever [`Config`] the file on disk already holds (or
    /// [`Config::default`] if there is none, or if what is there does not
    /// parse -- a config a later write is about to fix is not a reason to
    /// refuse the write), lets `mutate` change only the fields the caller
    /// actually cares about, and then serialises the *entire* resulting
    /// document with [`serde_json::to_string_pretty`]. Any hand-authored
    /// `"layouts"` entry or `"keymap"` override the caller did not touch
    /// therefore survives untouched -- a caller that wants to flip `theme`
    /// alone must not be able to accidentally erase a `"layouts"` section a
    /// user wrote by hand in an editor.
    ///
    /// [`PriceOverrides`](crate::infrastructure::pricing::overrides::PriceOverrides)
    /// has no equivalent write path to mirror here -- it is read-only, since
    /// nothing in this crate ever needs to correct a price on the user's
    /// behalf -- so the directory-creation behaviour below is new with this
    /// method rather than copied from it: [`std::fs::create_dir_all`] on the
    /// parent directory before writing, so the very first `merge_write` a
    /// fresh install ever makes does not fail merely because
    /// `~/.config/claude-stats` does not exist yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the parent directory cannot be created, the
    /// document cannot be serialised (never in practice: [`Config`] holds
    /// nothing `serde_json` cannot represent), or the file cannot be
    /// written.
    pub fn merge_write(&self, mutate: impl FnOnce(&mut Config)) -> anyhow::Result<()> {
        let mut config = match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Config::default(),
        };

        mutate(&mut config);

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(&config).context("cannot serialise the config")?;
        std::fs::write(&self.path, json)
            .with_context(|| format!("cannot write {}", self.path.display()))?;
        Ok(())
    }
}

/// Validates a freshly-loaded [`Config`]'s theme and layout names against
/// the real registries, downgrading an unresolvable name to `None` with a
/// warning rather than letting a typo like `"aurroa"` silently propagate
/// into a panic later at render time.
///
/// Kept apart from [`ConfigGateway::load_or_default`] on purpose: this is
/// the one step that needs a registry in hand, and threading one through
/// `load_or_default` would give a method whose only job is "read a file"
/// an opinion about `crate::tui::palette` it has no business holding. See
/// the module documentation's "Two different kinds of wrong" section.
#[must_use]
pub fn resolve(
    mut config: Config,
    themes: &crate::tui::palette::registry::ThemeRegistry,
) -> (Config, Vec<ConfigWarning>) {
    let mut warnings = Vec::new();

    if let Some(name) = config.theme.as_deref() {
        if themes.get(name).is_none() {
            warnings.push(ConfigWarning {
                message: format!("unknown theme {name:?}, using the default"),
            });
            config.theme = None;
        }
    }

    if let Some(name) = config.layout.as_deref() {
        let known = crate::tui::layout::presets::by_name(name).is_some()
            || config.layouts.contains_key(name);
        if !known {
            warnings.push(ConfigWarning {
                message: format!("unknown layout {name:?}, using the default"),
            });
            config.layout = None;
        }
    }

    (config, warnings)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A directory that deletes itself, so a test can write a real file
    /// without leaving one behind. Mirrors
    /// [`crate::infrastructure::pricing::overrides`]'s own test `TempDir`
    /// exactly, substituting `config.json` for `prices.json`.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "claude-stats-config-{name}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("a writable temporary directory");
            Self(dir)
        }

        /// Writes `contents` to `config.json` inside the directory and
        /// returns a gateway pointed at it.
        fn holding(&self, contents: &str) -> ConfigGateway {
            let path = self.0.join("config.json");
            std::fs::write(&path, contents).expect("a writable file");
            ConfigGateway::at(path)
        }

        /// A gateway pointed at `config.json` inside the directory, without
        /// writing anything there first.
        fn gateway(&self) -> ConfigGateway {
            ConfigGateway::at(self.0.join("config.json"))
        }

        fn missing_file(&self) -> ConfigGateway {
            ConfigGateway::at(self.0.join("nothing-here.json"))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_missing_config_file_behaves_identically_to_an_absent_one() {
        let dir = TempDir::new("absent");
        assert_eq!(
            dir.missing_file().load_or_default(),
            (Config::default(), None),
            "almost nobody will ever write this file; its absence is not a problem to report"
        );
    }

    #[test]
    fn a_malformed_config_file_falls_back_to_defaults_and_warns() {
        // A wrong theme name is cosmetic, not financial -- unlike a malformed
        // price override, this must never stop the dashboard from starting.
        let dir = TempDir::new("malformed");
        let gateway = dir.holding("{ \"theme\": ");

        let (config, warning) = gateway.load_or_default();

        assert_eq!(config, Config::default());
        let warning = warning.expect("a malformed file must not be silently ignored");
        assert!(
            warning.message.contains("config.json"),
            "the footer note should name the file: {}",
            warning.message
        );
    }

    #[test]
    fn an_unknown_theme_name_is_downgraded_to_none_with_a_warning() {
        let config = Config {
            theme: Some("not-a-real-theme".to_owned()),
            ..Config::default()
        };

        let (resolved, warnings) = resolve(
            config,
            crate::tui::palette::registry::ThemeRegistry::builtin(),
        );

        assert_eq!(resolved.theme, None);
        assert_eq!(warnings.len(), 1, "got {warnings:?}");
    }

    #[test]
    fn a_known_theme_name_survives_resolve_untouched() {
        let config = Config {
            theme: Some("aurora".to_owned()),
            ..Config::default()
        };

        let (resolved, warnings) = resolve(
            config,
            crate::tui::palette::registry::ThemeRegistry::builtin(),
        );

        assert_eq!(resolved.theme.as_deref(), Some("aurora"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn merge_write_preserves_untouched_fields() {
        let dir = TempDir::new("merge");
        let gateway = dir.gateway();

        gateway
            .merge_write(|config| {
                config.layouts.insert(
                    "custom".to_owned(),
                    LayoutNodeDto::Panel {
                        panel: "panel.spend-panel".to_owned(),
                    },
                );
            })
            .expect("the first write creates the directory and the file");

        gateway
            .merge_write(|config| {
                config.theme = Some("dracula".to_owned());
            })
            .expect("the second write only touches the theme");

        let (reloaded, warning) = gateway.load_or_default();
        assert!(warning.is_none(), "a file this gateway wrote must parse");
        assert_eq!(reloaded.theme.as_deref(), Some("dracula"));
        assert_eq!(
            reloaded.layouts.get("custom"),
            Some(&LayoutNodeDto::Panel {
                panel: "panel.spend-panel".to_owned()
            }),
            "the custom layout entry from the earlier write must survive untouched"
        );
    }

    #[test]
    fn a_layout_node_dto_naming_an_unregistered_panel_is_refused_rather_than_panicking() {
        let dto = LayoutNodeDto::Panel {
            panel: "panel.does-not-exist".to_owned(),
        };

        let error = crate::tui::layout::Node::try_from(dto)
            .expect_err("no panel is registered under this id");
        assert!(error.to_string().contains("panel.does-not-exist"));
    }

    #[test]
    fn a_layout_node_dto_for_a_registered_panel_converts_cleanly() {
        let dto = LayoutNodeDto::Panel {
            panel: "panel.spend-panel".to_owned(),
        };

        let node = crate::tui::layout::Node::try_from(dto).expect("a registered panel");
        assert_eq!(
            node,
            crate::tui::layout::Node::Panel {
                id: crate::tui::layout::PanelId("panel.spend-panel")
            }
        );
    }
}
