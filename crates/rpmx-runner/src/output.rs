//! Process-wide routing for guest output.
//!
//! Commands that promise machine-readable output on stdout must keep guest
//! stdout out of that stream. When this is enabled, both streams of every
//! step are written to stderr; the command's own JSON document stays the only
//! thing on stdout.

use std::sync::atomic::{AtomicBool, Ordering};

static LOGS_TO_STDERR: AtomicBool = AtomicBool::new(false);

/// Route every guest stream to stderr from now on.
pub fn set_logs_to_stderr(enabled: bool) {
    LOGS_TO_STDERR.store(enabled, Ordering::Relaxed);
}

/// Whether guest output belongs on stderr.
pub fn logs_to_stderr() -> bool {
    LOGS_TO_STDERR.load(Ordering::Relaxed)
}
