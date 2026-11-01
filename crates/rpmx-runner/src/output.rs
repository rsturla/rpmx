//! Process-wide routing for guest output.
//!
//! Commands that promise machine-readable output on stdout must keep guest
//! stdout out of that stream. When this is enabled, both streams of every
//! step are written to stderr; the command's own JSON document stays the only
//! thing on stdout.

use std::sync::atomic::{AtomicBool, Ordering};

static LOGS_TO_STDERR: AtomicBool = AtomicBool::new(false);
static KEEP_WORKDIR: AtomicBool = AtomicBool::new(false);
static DEBUG: AtomicBool = AtomicBool::new(false);
static JSON: AtomicBool = AtomicBool::new(false);

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
