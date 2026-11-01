use std::io::{Read, Write};
use std::num::NonZeroU64;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

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
    let stdout = child.stdout.take().map(|reader| pump(reader, false));
    let stderr = child.stderr.take().map(|reader| pump(reader, true));
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
        let _ = receiver.recv_timeout(Duration::from_secs(2));
    }
    Ok(Completion {
        status,
        elapsed: started.elapsed(),
    })
}

fn pump(mut reader: impl Read + Send + 'static, stderr: bool) -> Receiver<()> {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let mut sanitizer = Sanitizer::default();
        let mut clean = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    clean.clear();
                    sanitizer.push(&chunk[..read], &mut clean);
                    if stderr || crate::output::logs_to_stderr() {
                        let _ = std::io::stderr().write_all(&clean);
                    } else {
                        let _ = std::io::stdout().write_all(&clean);
                    }
                }
            }
        }
        let _ = done.send(());
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
