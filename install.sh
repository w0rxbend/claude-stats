#!/bin/sh
#
# Installer for claude-stats.
#
#   curl -fsSL https://raw.githubusercontent.com/w0rxbend/claude-stats/main/install.sh | sh
#
# Downloads the release binary for this machine, checks it against the
# published checksum, and puts it somewhere on PATH. Nothing is compiled and
# no toolchain is needed.
#
# Knobs, all optional:
#
#   CLAUDETUI_VERSION=v0.2.0   install a specific release instead of the latest
#   CLAUDETUI_INSTALL_DIR=...  install somewhere other than the default
#
# POSIX sh on purpose. The whole point of a one-line installer is that it runs
# before anything has been set up, so it cannot assume bash -- macOS ships an
# ancient one, and some containers ship none at all.

set -eu

REPO="w0rxbend/claude-stats"
BINARY="claude-stats"

# ── output ────────────────────────────────────────────────────────────

# Colour only when stderr is a terminal. Piping the installer into a log file
# should produce a log file, not a file full of escape sequences.
if [ -t 2 ]; then
    C_BOLD="$(printf '\033[1m')"
    C_DIM="$(printf '\033[2m')"
    C_RED="$(printf '\033[31m')"
    C_GREEN="$(printf '\033[32m')"
    C_CYAN="$(printf '\033[36m')"
    C_OFF="$(printf '\033[0m')"
else
    C_BOLD='' C_DIM='' C_RED='' C_GREEN='' C_CYAN='' C_OFF=''
fi

# Everything goes to stderr so that stdout stays clean for anyone piping this
# script's output somewhere.
say() { printf '%s\n' "$*" >&2; }
step() { printf '%s==>%s %s\n' "$C_CYAN" "$C_OFF" "$*" >&2; }
warn() { printf '%swarning:%s %s\n' "$C_BOLD" "$C_OFF" "$*" >&2; }

die() {
    printf '%serror:%s %s\n' "$C_RED" "$C_OFF" "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "this installer needs $1, which is not on PATH"
}

# ── which build do we want ────────────────────────────────────────────

# Maps this machine to one of the target triples the release publishes.
#
# Linux always gets the statically linked musl build, whatever libc the
# machine actually runs: it has no dynamic dependencies at all, so it works on
# glibc, musl and everything in between, and there is no version of this that
# needs a second Linux build.
detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux) os_part="unknown-linux-musl" ;;
        Darwin) os_part="apple-darwin" ;;
        MINGW* | MSYS* | CYGWIN*)
            die "Windows is not supported directly; install inside WSL 2 instead"
            ;;
        *) die "unsupported operating system: $os" ;;
    esac

    case "$arch" in
        x86_64 | amd64) arch_part="x86_64" ;;
        aarch64 | arm64) arch_part="aarch64" ;;
        *) die "unsupported architecture: $arch" ;;
    esac

    printf '%s-%s' "$arch_part" "$os_part"
}

# The newest published release tag.
#
# Read from the redirect that /releases/latest issues rather than from the
# JSON API: the redirect is unauthenticated, has a far more generous rate
# limit, and needs no JSON parser -- which matters because a machine fresh
# enough to need this installer may well not have jq.
latest_version() {
    curl -fsSLI -o /dev/null -w '%{url_effective}' \
        "https://github.com/$REPO/releases/latest" |
        sed 's|.*/tag/||'
}

# ── where to put it ───────────────────────────────────────────────────

# Picks an install directory, preferring one that needs no privileges.
#
# ~/.local/bin first, because installing a developer tool should never require
# sudo. /usr/local/bin is the fallback for the machines that do not use the
# XDG layout, and only if it is already writable -- silently escalating to
# sudo from inside a piped-in shell script is exactly the behaviour that makes
# people distrust one-line installers.
choose_install_dir() {
    if [ -n "${CLAUDETUI_INSTALL_DIR:-}" ]; then
        printf '%s' "$CLAUDETUI_INSTALL_DIR"
        return
    fi
    if [ -w "/usr/local/bin" ] && [ ! -d "$HOME/.local/bin" ]; then
        printf '/usr/local/bin'
        return
    fi
    printf '%s/.local/bin' "$HOME"
}

# Verifies one file against a line in a SHA256SUMS file.
#
# Linux and macOS ship different tools for this, and neither ships the other's,
# so both are handled. If neither is present the download is refused rather
# than installed unchecked: an unverified binary from the internet is the one
# outcome this script must not produce.
verify_checksum() {
    file="$1"
    sums="$2"
    name="$(basename "$file")"

    expected="$(awk -v n="$name" '$2 == n || $2 == "*" n { print $1 }' "$sums")"
    [ -n "$expected" ] || die "no checksum published for $name"

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$file" | cut -d' ' -f1)"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | cut -d' ' -f1)"
    else
        die "cannot verify the download: neither sha256sum nor shasum is available"
    fi

    [ "$actual" = "$expected" ] ||
        die "checksum mismatch for $name
  expected $expected
  got      $actual
This is not the file the release published. Nothing has been installed."
}

# ── main ──────────────────────────────────────────────────────────────

main() {
    need curl
    need tar
    need uname

    target="$(detect_target)"
    version="${CLAUDETUI_VERSION:-$(latest_version)}"
    [ -n "$version" ] || die "cannot determine the latest release; is $REPO published yet?"

    # Accept "0.2.0" as readily as "v0.2.0"; the tag carries the v.
    case "$version" in v*) ;; *) version="v$version" ;; esac

    archive="$BINARY-${version#v}-$target.tar.gz"
    base="https://github.com/$REPO/releases/download/$version"
    install_dir="$(choose_install_dir)"

    say ""
    say "  ${C_BOLD}claude-stats${C_OFF} installer"
    say "  ${C_DIM}version${C_OFF}  $version"
    say "  ${C_DIM}target${C_OFF}   $target"
    say "  ${C_DIM}into${C_OFF}     $install_dir"
    say ""

    # Everything happens in a temporary directory that is removed on any exit,
    # including a failed download -- a half-written binary left on PATH is
    # worse than no binary at all.
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT INT TERM

    # -fsL rather than -fsSL: curl's own "404" line adds nothing next to the
    # message below, and two errors for one problem reads like two problems.
    step "downloading $archive"
    curl -fsL "$base/$archive" -o "$tmp/$archive" ||
        die "no build published for $target at $version
See https://github.com/$REPO/releases for what is available."

    step "verifying the checksum"
    curl -fsL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" ||
        die "cannot download the checksum file for $version"
    verify_checksum "$tmp/$archive" "$tmp/SHA256SUMS"

    step "unpacking"
    tar -xzf "$tmp/$archive" -C "$tmp"
    binary="$tmp/${archive%.tar.gz}/$BINARY"
    [ -f "$binary" ] || die "the archive did not contain $BINARY"

    step "installing to $install_dir"
    mkdir -p "$install_dir" || die "cannot create $install_dir"
    # Install to a temporary name and rename into place. `mv` within a
    # filesystem is atomic, so a running claude-stats is never replaced halfway
    # through, and an interrupted install cannot leave a truncated binary
    # where a working one used to be.
    staged="$install_dir/.$BINARY.incoming.$$"
    cp "$binary" "$staged" || die "cannot write to $install_dir
Set CLAUDETUI_INSTALL_DIR to somewhere writable, or create $install_dir first."
    chmod 755 "$staged"
    mv -f "$staged" "$install_dir/$BINARY"

    say ""
    printf '%sinstalled%s %s/%s\n' "$C_GREEN" "$C_OFF" "$install_dir" "$BINARY" >&2

    case ":${PATH}:" in
        *":$install_dir:"*)
            say ""
            say "  run ${C_BOLD}$BINARY${C_OFF} next to a Claude Code session"
            ;;
        *)
            say ""
            warn "$install_dir is not on your PATH."
            say ""
            say "  Add it by appending this to your shell profile"
            say "  (~/.zshrc on macOS, ~/.bashrc on most Linux):"
            say ""
            say "    ${C_BOLD}export PATH=\"\$PATH:$install_dir\"${C_OFF}"
            ;;
    esac
    say ""
}

main "$@"
