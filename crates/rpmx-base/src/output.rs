//! Process-wide output policy and human-readable progress.
//!
//! Commands that promise machine-readable output on stdout must keep guest
//! stdout out of that stream. When this is enabled, both streams of every
//! step are written to stderr; the command's own JSON document stays the only
//! thing on stdout.
//!
//! Progress lines also go to stderr, so stdout remains the artifact list or
//! the single JSON document. Every crate reports through this module, which
//! keeps levels, phase names and alignment in one place.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

static LOGS_TO_STDERR: AtomicBool = AtomicBool::new(false);
static KEEP_WORKDIR: AtomicBool = AtomicBool::new(false);
static DEBUG: AtomicBool = AtomicBool::new(false);
static JSON: AtomicBool = AtomicBool::new(false);
static VERBOSE: AtomicBool = AtomicBool::new(false);
/// 0 = auto, 1 = always, 2 = never.
static COLOR: AtomicU8 = AtomicU8::new(0);

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

#[derive(Clone, Copy)]
enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERRO",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Level::Info => "\x1b[1;36m",
            Level::Warn => "\x1b[1;33m",
            Level::Error => "\x1b[1;31m",
        }
    }
}

/// Route every guest stream to stderr from now on.
pub fn set_logs_to_stderr(enabled: bool) {
    LOGS_TO_STDERR.store(enabled, Ordering::Relaxed);
}

/// Whether guest output belongs on stderr.
pub fn logs_to_stderr() -> bool {
    LOGS_TO_STDERR.load(Ordering::Relaxed)
}

/// Keep temporary work directories instead of deleting them.
pub fn set_keep_workdir(enabled: bool) {
    KEEP_WORKDIR.store(enabled, Ordering::Relaxed);
}

pub fn keep_workdir() -> bool {
    KEEP_WORKDIR.load(Ordering::Relaxed)
}

/// Echo step commands before they run.
pub fn set_debug(enabled: bool) {
    DEBUG.store(enabled, Ordering::Relaxed);
}

pub fn debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

/// Whether the command must emit exactly one machine-readable document.
pub fn set_json(enabled: bool) {
    JSON.store(enabled, Ordering::Relaxed);
}

pub fn json() -> bool {
    JSON.load(Ordering::Relaxed)
}

/// List resolved detail, such as every package in a build root.
pub fn set_verbose(enabled: bool) {
    VERBOSE.store(enabled, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// Select when progress lines use color: `auto`, `always` or `never`.
pub fn set_color(mode: &str) {
    let value = match mode {
        "always" => 1,
        "never" => 2,
        _ => 0,
    };
    COLOR.store(value, Ordering::Relaxed);
}

fn color_enabled() -> bool {
    match COLOR.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    }
}

fn write(level: Level, phase: &str, detail: impl std::fmt::Display) {
    if color_enabled() {
        eprintln!(
            "{}{}{} {BOLD}{phase:<8}{RESET} {detail}",
            level.color(),
            level.tag(),
            RESET
        );
    } else {
        eprintln!("{:<4} {phase:<8} {detail}", level.tag());
    }
}

/// Write one aligned progress line to stderr.
///
/// Progress never goes to stdout: that stream is reserved for the artifact
/// list, or for the single document `--json` promises.
pub fn phase(phase: &str, detail: impl std::fmt::Display) {
    write(Level::Info, phase, detail);
}

/// Format one phase line without color or writing it, so the layout is
/// testable.
pub fn phase_line(phase: &str, detail: impl std::fmt::Display) -> String {
    format!("{:<4} {phase:<8} {detail}", Level::Info.tag())
}

/// Write an indented line under the preceding phase, used by `--verbose`.
pub fn item(detail: impl std::fmt::Display) {
    write(Level::Info, "", detail);
}

/// Report something the user should look at but that does not fail the build.
pub fn warning(phase: &str, detail: impl std::fmt::Display) {
    write(Level::Warn, phase, detail);
}

/// Report a terminal error.
pub fn error(detail: impl std::fmt::Display) {
    if color_enabled() {
        eprintln!("{}{}{} {detail}", Level::Error.color(), Level::Error.tag(), RESET);
    } else {
        eprintln!("{} {detail}", Level::Error.tag());
    }
}

/// Report one artifact where it is produced, using a path relative to the
/// current directory when possible.
pub fn wrote(path: &std::path::Path) {
    phase("wrote", display_path(path));
}

/// Report one package entering a build root.
pub fn installing(identity: impl std::fmt::Display) {
    phase("install", identity);
}

/// Render a path relative to the current directory when it is below it.
pub fn display_path(path: &std::path::Path) -> String {
    let Ok(current) = std::env::current_dir() else {
        return path.display().to_string();
    };
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let current = current.canonicalize().unwrap_or(current);
    match canonical.strip_prefix(&current) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.display().to_string(),
        _ => path.display().to_string(),
    }
}

/// Format a count with the noun that agrees with it.
pub fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Format a duration for progress lines.
pub fn duration(started: std::time::Instant) -> String {
    format!("{:.1}s", started.elapsed().as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::{count, phase_line};

    #[test]
    fn counts_use_the_matching_noun() {
        assert_eq!(count(0, "artifact", "artifacts"), "0 artifacts");
        assert_eq!(count(1, "artifact", "artifacts"), "1 artifact");
        assert_eq!(count(2, "package", "packages"), "2 packages");
    }

    #[test]
    fn phase_lines_align_their_detail() {
        assert_eq!(
            phase_line("source", "RPMX-SOURCE.tar.gz"),
            "INFO source   RPMX-SOURCE.tar.gz"
        );
        assert_eq!(phase_line("package", "hello"), "INFO package  hello");
        assert_eq!(
            phase_line("wrote", "/tmp/out/a.rpm"),
            "INFO wrote    /tmp/out/a.rpm"
        );
        assert_eq!(
            phase_line("install", "glibc-2.44.x86_64"),
            "INFO install  glibc-2.44.x86_64"
        );
    }
}
