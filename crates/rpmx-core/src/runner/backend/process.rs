use std::num::NonZeroU64;
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

use crate::error::{codes, Error, Result};

pub(crate) struct Completion {
    status: ExitStatus,
    elapsed: Duration,
}

pub(crate) fn spawn(
    program: &str,
    args: &[String],
    timeout: NonZeroU64,
    label: &str,
) -> Result<Completion> {
    let mut process = Command::new("timeout");
    process
        .arg("--signal=TERM")
        .arg("--kill-after=5")
        .arg(timeout.get().to_string())
        .arg(program);
    process.args(args);
    let started = Instant::now();
    let status = process.status().map_err(|error| {
        Error::new(
            codes::SANDBOX_001,
            format!("cannot start {program} for '{label}': {error}"),
        )
    })?;
    Ok(Completion {
        status,
        elapsed: started.elapsed(),
    })
}

pub(crate) fn timed_out(completion: &Completion, timeout: NonZeroU64) -> bool {
    completion.elapsed >= Duration::from_secs(timeout.get()) && !completion.status.success()
}

pub(crate) fn finish(
    completion: Completion,
    timeout: NonZeroU64,
    label: &str,
    prefix: &str,
) -> Result<()> {
    if timed_out(&completion, timeout) {
        return Err(Error::new(
            codes::SANDBOX_004,
            format!("{prefix}step '{label}' timed out after {}s", timeout.get()),
        ));
    }
    if !completion.status.success() {
        return Err(Error::new(
            codes::SANDBOX_002,
            format!(
                "{prefix}step '{label}' failed with {}",
                completion
                    .status
                    .code()
                    .map_or_else(|| "a signal".to_string(), |code| format!("exit {code}"))
            ),
        ));
    }
    Ok(())
}
