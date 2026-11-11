use std::io::{Read, Write};
use std::num::NonZeroU64;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::runner::OutputLogs;
use crate::terminal::Sanitizer;
use rpmx_base::error::{codes, Error, Result};

pub(crate) struct Completion {
    status: ExitStatus,
    elapsed: Duration,
}

pub(crate) fn spawn(
    program: &str,
    args: &[String],
    timeout: NonZeroU64,
    label: &str,
    logs: Option<&OutputLogs>,
) -> Result<Completion> {
    let mut process = Command::new("timeout");
    process
        .arg("--signal=TERM")
        .arg("--kill-after=5")
        .arg(timeout.get().to_string())
        .arg(program);
    process.args(args);
    let started = Instant::now();
    let mut child = process
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            Error::new(
                codes::SANDBOX_001,
                format!("cannot start {program} for '{label}': {error}"),
            )
        })?;
    let stdout_log = logs
        .map(|logs| logs.stdout.try_clone())
        .transpose()
        .map_err(log_error)?;
    let stderr_log = logs
        .map(|logs| logs.stderr.try_clone())
        .transpose()
        .map_err(log_error)?;
    let arch = rpmx_base::output::current_arch();
    let stdout = child
        .stdout
        .take()
        .map(|reader| pump(reader, false, arch.clone(), stdout_log));
    let stderr = child
        .stderr
        .take()
        .map(|reader| pump(reader, true, arch, stderr_log));
    let status = child.wait().map_err(|error| {
        Error::new(
            codes::SANDBOX_001,
            format!("cannot wait for {program} for '{label}': {error}"),
        )
    })?;
    // A step can daemonize a process that keeps the pipes open. The timeout
    // has already killed the direct child; do not let an orphan defeat it by
    // blocking these joins forever.
    for receiver in [stdout, stderr].into_iter().flatten() {
        if let Ok(result) = receiver.recv_timeout(Duration::from_secs(2)) {
            result.map_err(log_error)?;
        }
    }
    Ok(Completion {
        status,
        elapsed: started.elapsed(),
    })
}

fn log_error(error: std::io::Error) -> Error {
    Error::new(
        codes::SANDBOX_001,
        format!("cannot record test output: {error}"),
    )
}

fn pump(
    mut reader: impl Read + Send + 'static,
    stderr: bool,
    arch: Option<String>,
    mut log: Option<std::fs::File>,
) -> Receiver<std::io::Result<()>> {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        rpmx_base::output::set_arch(arch.as_deref());
        let mut sanitizer = Sanitizer::default();
        let mut clean = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut guest = rpmx_base::output::logs_to_stderr()
            .then(|| rpmx_base::output::GuestWriter::new(stderr));
        let mut error = None;
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Err(read) => {
                    error.get_or_insert(read);
                    break;
                }
                Ok(read) => {
                    clean.clear();
                    sanitizer.push(&chunk[..read], &mut clean);
                    if let Some(log) = &mut log {
                        if let Err(write) = log.write_all(&clean) {
                            error.get_or_insert(write);
                        }
                    }
                    match &mut guest {
                        Some(writer) => writer.push(&clean),
                        None if stderr => {
                            let _ = std::io::stderr().write_all(&clean);
                        }
                        None => {
                            let _ = std::io::stdout().write_all(&clean);
                        }
                    }
                }
            }
        }
        if let Some(writer) = &mut guest {
            writer.flush();
        }
        let _ = done.send(error.map_or(Ok(()), Err));
    });
    finished
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

#[cfg(test)]
mod tests {
    use super::spawn;
    use std::num::NonZeroU64;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn a_daemonized_child_cannot_defeat_the_timeout() {
        if Command::new("setsid").arg("--version").output().is_err() {
            eprintln!("setsid not installed; skipping daemon timeout test");
            return;
        }
        let started = Instant::now();
        let completion = spawn(
            "sh",
            &[
                "-c".to_string(),
                "setsid sleep 5 & exec sleep 5".to_string(),
            ],
            NonZeroU64::new(1).expect("timeout"),
            "daemon",
            None,
        )
        .expect("spawn");
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "spawn waited {:?} for a daemonized child",
            started.elapsed()
        );
        assert!(!completion.status.success());
    }
}
