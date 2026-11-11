use std::io::{Read, Write};
use std::num::NonZeroU64;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
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
    spawn_with_fds(program, args, timeout, label, logs, &[])
}

// Change descriptor inheritance only after fork. Clearing CLOEXEC in the
// parent would leak namespace handles into unrelated concurrent commands.
pub(crate) fn spawn_inheriting_fds(
    mut command: Command,
    descriptors: &[BorrowedFd<'_>],
) -> std::io::Result<Child> {
    let descriptors: Vec<_> = descriptors.iter().map(AsRawFd::as_raw_fd).collect();
    // SAFETY: the child hook uses only async-signal-safe fcntl and errno access.
    unsafe {
        command.pre_exec(move || {
            for descriptor in &descriptors {
                let flags = libc::fcntl(*descriptor, libc::F_GETFD);
                if flags < 0
                    || libc::fcntl(*descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    // Spawn while every borrowed descriptor is still alive, and consume the
    // Command so its raw descriptor hook cannot be reused after this call.
    command.spawn()
}

pub(crate) fn spawn_with_fds(
    program: &str,
    args: &[String],
    timeout: NonZeroU64,
    label: &str,
    logs: Option<&OutputLogs>,
    descriptors: &[BorrowedFd<'_>],
) -> Result<Completion> {
    let mut process = Command::new("timeout");
    process
        .arg("--signal=TERM")
        .arg("--kill-after=5")
        .arg(timeout.get().to_string())
        .arg(program);
    process.args(args);
    process.stdout(Stdio::piped()).stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = spawn_inheriting_fds(process, descriptors).map_err(|error| {
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
    use super::{spawn, spawn_inheriting_fds};
    use std::fs::File;
    use std::io::{Read, Write};
    use std::num::NonZeroU64;
    use std::os::fd::{AsFd, AsRawFd};
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn selected_fd_inheritance_does_not_change_parent_or_unrelated_execs() {
        let namespace = File::open("/proc/self/ns/user").unwrap();
        let (control, peer) = UnixStream::pair().unwrap();
        let namespace_number = namespace.as_raw_fd().to_string();
        let control_number = peer.as_raw_fd().to_string();
        let target = |number: &str| {
            std::fs::read_link(format!("/proc/self/fd/{number}"))
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        let namespace_target = target(&namespace_number);
        let control_target = target(&control_number);
        let parent_descriptors = [namespace.as_raw_fd(), peer.as_raw_fd(), control.as_raw_fd()];
        let parent_flags = || {
            for descriptor in parent_descriptors {
                // SAFETY: F_GETFD only inspects an open descriptor owned here.
                assert_ne!(
                    unsafe { libc::fcntl(descriptor, libc::F_GETFD) } & libc::FD_CLOEXEC,
                    0
                );
            }
        };
        parent_flags();
        let arguments = [
            &namespace_number,
            &namespace_target,
            &control_number,
            &control_target,
        ];
        let mut selected = Command::new("/bin/sh");
        selected
            .args([
                "-ec",
                r#"
            test "$(readlink /proc/$$/fd/$1)" = "$2"
            test "$(readlink /proc/$$/fd/$3)" = "$4"
            printf R
            IFS= read -r done
        "#,
                "selected-fd-test",
            ])
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut selected =
            spawn_inheriting_fds(selected, &[namespace.as_fd(), peer.as_fd()]).unwrap();
        let mut ready = [0];
        selected
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        assert_eq!(ready, *b"R");
        parent_flags();
        // Run while the selected child still holds the same handles. A global
        // CLOEXEC change would expose them to this unrelated concurrent exec.
        let unrelated = Command::new("/bin/sh")
            .args([
                "-ec",
                r#"
            test "$(readlink /proc/$$/fd/$1 2>/dev/null || :)" != "$2"
            test "$(readlink /proc/$$/fd/$3 2>/dev/null || :)" != "$4"
        "#,
                "unrelated-fd-test",
            ])
            .args(arguments)
            .status()
            .unwrap();
        assert!(unrelated.success());
        selected
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"done\n")
            .unwrap();
        assert!(selected.wait().unwrap().success());
        parent_flags();
    }

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
