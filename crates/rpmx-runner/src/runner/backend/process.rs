use std::io::{Read, Write};
#[cfg(test)]
use std::num::NonZeroU64;
use std::os::fd::BorrowedFd;
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

#[cfg(test)]
pub(crate) fn spawn(
    program: &str,
    args: &[String],
    timeout: NonZeroU64,
    label: &str,
    logs: Option<&OutputLogs>,
) -> Result<Completion> {
    spawn_with_fds(program, args, timeout, label, logs, &[])
}

pub(crate) use rpmx_base::process::spawn_sealed as spawn_inheriting_fds;

#[cfg(test)]
pub(crate) fn spawn_with_fds(
    program: &str,
    args: &[String],
    timeout: NonZeroU64,
    label: &str,
    logs: Option<&OutputLogs>,
    descriptors: &[BorrowedFd<'_>],
) -> Result<Completion> {
    spawn_with_fds_for(
        program,
        args,
        Duration::from_secs(timeout.get()),
        label,
        logs,
        descriptors,
    )
}

pub(crate) fn spawn_with_fds_for(
    program: &str,
    args: &[String],
    timeout: Duration,
    label: &str,
    logs: Option<&OutputLogs>,
    descriptors: &[BorrowedFd<'_>],
) -> Result<Completion> {
    if timeout.is_zero() {
        return Err(Error::new(
            codes::SANDBOX_004,
            format!("step '{label}' has no time remaining"),
        ));
    }
    let mut process = Command::new("timeout");
    process
        .arg("--signal=TERM")
        .arg("--kill-after=5")
        .arg(format!("{:.9}s", timeout.as_secs_f64()))
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

pub(crate) fn timed_out_for(completion: &Completion, timeout: Duration) -> bool {
    completion.elapsed >= timeout && !completion.status.success()
}

pub(crate) fn finish_for(
    completion: Completion,
    timeout: Duration,
    label: &str,
    prefix: &str,
) -> Result<()> {
    if timed_out_for(&completion, timeout) {
        return Err(Error::new(
            codes::SANDBOX_004,
            format!(
                "{prefix}step '{label}' timed out after {:.3}s",
                timeout.as_secs_f64()
            ),
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
                    .map_or_else(|| "a signal".into(), |code| format!("exit {code}"))
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
    #[cfg(test)]
    use std::num::NonZeroU64;
    use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn inherited_pipes_and_files_are_sealed_without_changing_the_parent() {
        const CHILD: &str = "RPMX_DESCRIPTOR_ISOLATION_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Intentionally inheritable descriptors must not be created in
            // the multithreaded test parent, where unrelated tests could race.
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "runner::backend::process::tests::inherited_pipes_and_files_are_sealed_without_changing_the_parent",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let output = spawn_inheriting_fds(command, &[])
                .unwrap()
                .wait_with_output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let mut pipe = [-1; 2];
        // SAFETY: pipe points to storage for both returned descriptors.
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        // SAFETY: a successful pipe2 returned two distinct owned descriptors.
        let (pipe_a, pipe_b) =
            unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
        let private = tempfile::tempfile().unwrap();
        let inherited: Vec<OwnedFd> = [
            (pipe_a.as_raw_fd(), 142),
            (pipe_b.as_raw_fd(), 145),
            (private.as_raw_fd(), 151),
        ]
        .into_iter()
        .map(|(source, minimum)| {
            // SAFETY: F_DUPFD returns a new owned, non-CLOEXEC descriptor.
            let duplicated = unsafe { libc::fcntl(source, libc::F_DUPFD, minimum) };
            assert!(duplicated >= minimum);
            unsafe { OwnedFd::from_raw_fd(duplicated) }
        })
        .collect();
        assert_eq!(inherited[0].as_raw_fd(), 142);
        assert_eq!(inherited[1].as_raw_fd(), 145);
        let allowed = File::open("/proc/self/ns/user").unwrap();
        let target = |fd| std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap();
        let initial_targets: Vec<_> = inherited.iter().map(|fd| target(fd.as_raw_fd())).collect();
        let allowed_target = target(allowed.as_raw_fd());
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-ec",
                r#"
                for descriptor in 142 145 151; do
                    if test -e "/proc/$$/fd/$descriptor"; then
                        echo "unexpected inherited descriptor $descriptor" >&2
                        exit 1
                    fi
                done
                test "$(readlink /proc/$$/fd/$1)" = "$2"
                IFS= read -r input
                test "$input" = inherited-stdin
                printf 'isolated-stdout\n'
                printf 'isolated-stderr\n' >&2
            "#,
                "descriptor-isolation-test",
                &allowed.as_raw_fd().to_string(),
                allowed_target.to_str().unwrap(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn_inheriting_fds(command, &[allowed.as_fd()]).unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"inherited-stdin\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"isolated-stdout\n");
        assert_eq!(output.stderr, b"isolated-stderr\n");
        for (descriptor, initial) in inherited.iter().zip(initial_targets) {
            assert_eq!(target(descriptor.as_raw_fd()), initial);
            // SAFETY: every descriptor remains owned and open in this parent.
            assert_eq!(
                unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) },
                0
            );
        }
        // The exec-error pipe must remain usable until an unsuccessful exec.
        let error =
            spawn_inheriting_fds(Command::new("/rpmx-no-such-executable"), &[]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        let mut invalid = Command::new("/bin/true");
        // SAFETY: the hook only returns a fixed exec/setup-style error.
        unsafe {
            invalid.pre_exec(|| Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
        }
        let error = spawn_inheriting_fds(invalid, &[]).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
        assert!(!error.to_string().contains("close_range"), "{error}");

        // Emulate an unavailable syscall only in this dedicated subprocess.
        // The kernel, rather than a mocked return value, rejects close_range.
        let filters = [
            libc::sock_filter {
                code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
                jt: 0,
                jf: 0,
                k: 0,
            },
            libc::sock_filter {
                code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                jt: 0,
                jf: 1,
                k: libc::SYS_close_range as u32,
            },
            libc::sock_filter {
                code: (libc::BPF_RET | libc::BPF_K) as u16,
                jt: 0,
                jf: 0,
                k: libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32,
            },
            libc::sock_filter {
                code: (libc::BPF_RET | libc::BPF_K) as u16,
                jt: 0,
                jf: 0,
                k: libc::SECCOMP_RET_ALLOW,
            },
        ];
        let filter = libc::sock_fprog {
            len: filters.len() as u16,
            filter: filters.as_ptr().cast_mut(),
        };
        // SAFETY: both prctl arguments match the documented filter ABI and
        // point to live storage. This subprocess runs no other tests.
        unsafe {
            assert_eq!(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), 0);
            assert_eq!(
                libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &filter),
                0
            );
        }
        let error = spawn_inheriting_fds(Command::new("/bin/true"), &[]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("Linux 5.11+"), "{error}");
        assert!(
            error
                .to_string()
                .contains("close_range(CLOSE_RANGE_CLOEXEC)"),
            "{error}"
        );
    }

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
