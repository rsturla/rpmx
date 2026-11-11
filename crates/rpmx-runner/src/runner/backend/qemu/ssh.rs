use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::runner::RunnerRequest;
use crate::terminal::Sanitizer;
use rpmx_base::error::{codes, Error, Result};

/// Keep error diagnostics useful without letting a hostile or noisy guest
/// grow host memory without bound. The last `STDERR_LIMIT` bytes are retained;
/// everything older is drained and discarded.
const STDERR_LIMIT: usize = 64 * 1024;

/// The guest is untrusted output, not an operator-sized input. Cap what a
/// sync-out can stage even though SRPM/RPM/source sizes are uncapped.
pub(crate) const MAX_GUEST_TRANSFER_BYTES: u64 = 8 * 1024 * 1024 * 1024;

fn retain_tail(buffer: &mut Vec<u8>, bytes: &[u8]) {
    buffer.extend_from_slice(bytes);
    if buffer.len() > STDERR_LIMIT {
        let overflow = buffer.len() - STDERR_LIMIT;
        buffer.drain(..overflow);
    }
}

fn take_stderr(child: &mut Child) -> Option<Receiver<std::io::Result<Vec<u8>>>> {
    child.stderr.take().map(|mut stderr| {
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut clean = Vec::new();
            let mut sanitizer = Sanitizer::default();
            let mut chunk = [0u8; 8192];
            let result = loop {
                match stderr.read(&mut chunk) {
                    Ok(0) => break Ok(buffer),
                    Ok(read) => {
                        clean.clear();
                        sanitizer.push(&chunk[..read], &mut clean);
                        retain_tail(&mut buffer, &clean);
                    }
                    Err(error) => break Err(error),
                }
            };
            let _ = done.send(result);
        });
        finished
    })
}

pub(super) fn take_stderr_with_echo(
    child: &mut Child,
    mut log: Option<std::fs::File>,
) -> Option<Receiver<std::io::Result<Vec<u8>>>> {
    child.stderr.take().map(|mut stderr| {
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut clean = Vec::new();
            let mut sanitizer = Sanitizer::default();
            let mut chunk = [0u8; 8192];
            let mut write_error = None;
            let result = loop {
                match stderr.read(&mut chunk) {
                    Ok(0) => break write_error.map_or(Ok(buffer), Err),
                    Ok(read) => {
                        clean.clear();
                        sanitizer.push(&chunk[..read], &mut clean);
                        let _ = std::io::stderr().write_all(&clean);
                        if let Some(log) = &mut log {
                            if let Err(error) = log.write_all(&clean) {
                                write_error.get_or_insert(error);
                            }
                        }
                        retain_tail(&mut buffer, &clean);
                    }
                    Err(error) => break Err(error),
                }
            };
            let _ = done.send(result);
        });
        finished
    })
}

pub(super) fn take_stdout_echo(
    child: &mut Child,
    mut log: Option<std::fs::File>,
) -> Option<Receiver<std::io::Result<()>>> {
    child.stdout.take().map(|mut stdout| {
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let mut out: Box<dyn std::io::Write + Send> = if rpmx_base::output::logs_to_stderr() {
                Box::new(std::io::stderr())
            } else {
                Box::new(std::io::stdout())
            };
            let mut error = None;
            let mut clean = Vec::new();
            let mut sanitizer = Sanitizer::default();
            let mut chunk = [0u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        clean.clear();
                        sanitizer.push(&chunk[..read], &mut clean);
                        if let Some(log) = &mut log {
                            if let Err(write) = log.write_all(&clean) {
                                error.get_or_insert(write);
                            }
                        }
                        if let Err(write) = out.write_all(&clean) {
                            error.get_or_insert(write);
                        }
                    }
                    Err(read) => {
                        error.get_or_insert(read);
                        break;
                    }
                }
            }
            if let Err(flush) = out.flush() {
                error.get_or_insert(flush);
            }
            let result = match error {
                Some(error) => Err(error),
                None => Ok(()),
            };
            let _ = done.send(result);
        });
        finished
    })
}

pub(super) fn join_stderr(handle: Option<Receiver<std::io::Result<Vec<u8>>>>) -> Result<Vec<u8>> {
    let Some(handle) = handle else {
        return Ok(Vec::new());
    };
    match handle.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(buffer)) => Ok(buffer),
        Ok(Err(error)) => Err(Error::new(
            codes::RUNNER_003,
            format!("cannot read ssh stderr: {error}"),
        )),
        // A daemonized guest process may keep the pipe open after the timeout
        // killed ssh. Report without diagnostics rather than hanging.
        Err(_) => Ok(Vec::new()),
    }
}

fn exit_reason(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "a signal".to_string(), |code| format!("exit {code}"))
}

fn remote_detail(stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        "no diagnostics".to_string()
    } else {
        detail.to_string()
    }
}

pub(super) fn stream_tar_in(mut child: Child, source: &Path, label: &str) -> Result<()> {
    let stderr_reader = take_stderr(&mut child);
    // `tar -x` is normally silent, but drain stdout anyway so a chatty remote
    // command cannot block on a full pipe while we are writing the archive.
    let stdout_reader = child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        })
    });
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::new(codes::RUNNER_003, "ssh stdin is not available for copy-in"))?;
    let archive = (|| -> std::io::Result<()> {
        let mut builder = tar::Builder::new(&mut stdin);
        builder.follow_symlinks(false);
        builder.append_dir_all(".", source)?;
        builder.finish()
    })();
    // Closing stdin is the abort signal: the remote tar sees EOF. Do not kill
    // the direct child here - when a timeout is configured it is the `timeout`
    // wrapper, and killing it would strand ssh and disable the watchdog.
    drop(stdin);
    let status = child
        .wait()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("ssh failed: {error}")))?;
    let stderr = join_stderr(stderr_reader)?;
    drop(stdout_reader);
    if let Err(error) = &archive {
        let remote = if stderr.is_empty() {
            String::new()
        } else {
            format!("; remote: {}", remote_detail(&stderr))
        };
        return Err(Error::new(
            codes::RUNNER_003,
            format!("cannot archive {}: {error}{remote}", source.display()),
        ));
    }
    if !status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "cannot copy {label} into qemu ({}): {}",
                exit_reason(status),
                remote_detail(&stderr)
            ),
        ));
    }
    archive.map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot archive {}: {error}", source.display()),
        )
    })?;
    Ok(())
}

fn copy_archive(
    reader: &mut impl Read,
    archive: &mut std::fs::File,
    remote: &str,
    host: &Path,
) -> Result<()> {
    copy_archive_with_limit(reader, archive, remote, host, MAX_GUEST_TRANSFER_BYTES)
}

fn copy_archive_with_limit(
    reader: &mut impl Read,
    archive: &mut std::fs::File,
    remote: &str,
    host: &Path,
    limit: u64,
) -> Result<()> {
    let mut chunk = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut chunk).map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot read {remote} from qemu: {error}"),
            )
        })?;
        if read == 0 {
            return Ok(());
        }
        total = total.saturating_add(read as u64);
        if total > limit {
            return Err(Error::new(
                codes::RUNNER_003,
                format!("archive from {remote} exceeds the {limit}-byte guest transfer limit"),
            ));
        }
        archive.write_all(&chunk[..read]).map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!(
                    "cannot write the archive for {remote} in {}: {error}",
                    host.display()
                ),
            )
        })?;
    }
}

pub(super) fn stream_tar_out(
    mut child: Child,
    remote: &str,
    host: &Path,
    mut archive: std::fs::File,
) -> Result<()> {
    let stderr_reader = take_stderr(&mut child);
    let mut stdout = child.stdout.take().ok_or_else(|| {
        Error::new(
            codes::RUNNER_003,
            "ssh stdout is not available for sync-out",
        )
    })?;
    let copy = copy_archive(&mut stdout, &mut archive, remote, host);
    // Closing stdout is the abort signal: ssh sees EPIPE if the guest is still
    // writing. Do not kill the direct child here, for the timeout reason above.
    drop(stdout);
    let status = child
        .wait()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("ssh failed: {error}")))?;
    let stderr = join_stderr(stderr_reader)?;
    if let Err(error) = copy {
        if status.success() {
            return Err(error);
        }
        let remote = if stderr.is_empty() {
            format!(" ({})", exit_reason(status))
        } else {
            format!(" ({}): {}", exit_reason(status), remote_detail(&stderr))
        };
        return Err(Error::new(
            codes::RUNNER_003,
            format!("{}{remote}", error.message),
        ));
    }
    if !status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "cannot read {remote} from qemu ({}): {}",
                exit_reason(status),
                remote_detail(&stderr)
            ),
        ));
    }
    rpmx_store::tar_guard::unpack_staged_with(
        archive,
        host,
        &format!("archive from {remote}"),
        codes::RUNNER_003,
        Some(MAX_GUEST_TRANSFER_BYTES),
        0,
        "",
    )
}

pub(super) fn stream_command_output(
    mut child: Child,
    stdin: Option<&[u8]>,
    logs: Option<&crate::runner::OutputLogs>,
) -> Result<(ExitStatus, Vec<u8>)> {
    let stdout_log = logs
        .map(|logs| logs.stdout.try_clone())
        .transpose()
        .map_err(|error| Error::new(codes::RUNNER_003, error.to_string()))?;
    let stderr_log = logs
        .map(|logs| logs.stderr.try_clone())
        .transpose()
        .map_err(|error| Error::new(codes::RUNNER_003, error.to_string()))?;
    let stdout = take_stdout_echo(&mut child, stdout_log);
    let stderr = take_stderr_with_echo(&mut child, stderr_log);
    let mut write_error = None;
    if let Some(bytes) = stdin {
        match child.stdin.take() {
            Some(mut input) => {
                if let Err(error) = input.write_all(bytes) {
                    write_error = Some(error);
                }
            }
            None => {
                write_error = Some(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "ssh stdin is not available",
                ));
            }
        }
    }
    let status = child
        .wait()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("ssh failed: {error}")))?;
    let stderr = join_stderr(stderr)?;
    if let Some(stdout) = stdout {
        if let Ok(result) = stdout.recv_timeout(Duration::from_secs(2)) {
            result.map_err(|error| {
                Error::new(
                    codes::RUNNER_003,
                    format!("cannot record ssh stdout: {error}"),
                )
            })?;
        }
    }
    if let Some(error) = write_error {
        if status.success() {
            return Err(Error::new(
                codes::RUNNER_003,
                format!("cannot write to ssh: {error}"),
            ));
        }
    }
    Ok((status, stderr))
}

pub(super) fn step_script(request: &RunnerRequest<'_>) -> String {
    let mut script = String::from("#!/bin/sh\nset +e\n");
    script.push_str(&format!("cd {}\n", shell_quote(request.cwd)));
    for (key, value) in request.env {
        script.push_str(&format!("export {}={}\n", key, shell_quote(value)));
    }
    script.push_str(&shell_quote(request.command));
    for arg in request.args {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    script.push_str("\nexit $?\n");
    script
}

pub(super) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let mut out = String::from("'");
    for character in value.chars() {
        if character == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(character);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::stream_command_output;
    use std::process::{Command, Stdio};

    fn piped(command: &str) -> std::process::Child {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn test transport")
    }

    #[test]
    fn guest_archive_transfer_is_capped() {
        let root = tempfile::tempdir().expect("work");
        let path = root.path().join("archive");
        let mut archive = std::fs::File::create(&path).expect("archive");
        let mut reader = std::io::Cursor::new(vec![0u8; 100]);
        let error =
            super::copy_archive_with_limit(&mut reader, &mut archive, "remote", root.path(), 8)
                .expect_err("transfer cap");
        assert!(
            error.message.contains("guest transfer limit"),
            "{}",
            error.message
        );
    }

    #[test]
    fn step_stderr_is_captured_for_the_error_message() {
        let child = piped("printf 'warning\\n' >&2");
        let (status, stderr) = stream_command_output(child, None, None).expect("stream output");
        assert!(status.success());
        assert_eq!(stderr, b"warning\n");
    }

    #[test]
    fn step_logs_preserve_both_streams() {
        let directory = tempfile::tempdir().unwrap();
        let logs = crate::runner::OutputLogs {
            stdout: std::fs::File::create(directory.path().join("stdout")).unwrap(),
            stderr: std::fs::File::create(directory.path().join("stderr")).unwrap(),
        };
        let child = piped("printf 'output\\n'; printf 'warning\\n' >&2");
        let (status, _) = stream_command_output(child, None, Some(&logs)).unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read(directory.path().join("stdout")).unwrap(),
            b"output\n"
        );
        assert_eq!(
            std::fs::read(directory.path().join("stderr")).unwrap(),
            b"warning\n"
        );
    }

    #[test]
    fn step_stdin_is_forwarded() {
        let child = piped("wc -c >/dev/null");
        let payload = vec![b'x'; 256 * 1024];
        let (status, _) = stream_command_output(child, Some(&payload), None).expect("stream stdin");
        assert!(status.success());
    }
}
