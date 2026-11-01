use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::thread::JoinHandle;

use crate::error::{codes, Error, Result};
use crate::runner::RunnerRequest;

/// Keep error diagnostics useful without letting a hostile or noisy guest
/// grow host memory without bound. The rest is drained and discarded.
const STDERR_LIMIT: usize = 64 * 1024;

fn take_stderr(child: &mut Child) -> Option<JoinHandle<std::io::Result<Vec<u8>>>> {
    child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let read = stderr.read(&mut chunk)?;
                if read == 0 {
                    break;
                }
                let remaining = STDERR_LIMIT.saturating_sub(buffer.len());
                if remaining > 0 {
                    buffer.extend_from_slice(&chunk[..read.min(remaining)]);
                }
            }
            Ok(buffer)
        })
    })
}

pub(super) fn take_stderr_with_echo(
    child: &mut Child,
) -> Option<JoinHandle<std::io::Result<Vec<u8>>>> {
    child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let read = stderr.read(&mut chunk)?;
                if read == 0 {
                    break;
                }
                let _ = std::io::stderr().write_all(&chunk[..read]);
                let remaining = STDERR_LIMIT.saturating_sub(buffer.len());
                if remaining > 0 {
                    buffer.extend_from_slice(&chunk[..read.min(remaining)]);
                }
            }
            Ok(buffer)
        })
    })
}

pub(super) fn take_stdout_echo(child: &mut Child) -> Option<JoinHandle<std::io::Result<()>>> {
    child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            let mut out = std::io::stdout().lock();
            let mut error = None;
            let mut chunk = [0u8; 8192];
            loop {
                let read = stdout.read(&mut chunk)?;
                if read == 0 {
                    break;
                }
                if let Err(write) = out.write_all(&chunk[..read]) {
                    error.get_or_insert(write);
                }
            }
            if let Err(flush) = out.flush() {
                error.get_or_insert(flush);
            }
            match error {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
    })
}

pub(super) fn join_stderr(handle: Option<JoinHandle<std::io::Result<Vec<u8>>>>) -> Result<Vec<u8>> {
    let Some(handle) = handle else {
        return Ok(Vec::new());
    };
    handle
        .join()
        .map_err(|_| Error::new(codes::RUNNER_003, "ssh stderr reader panicked"))?
        .map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot read ssh stderr: {error}"),
            )
        })
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
    if let Some(reader) = stdout_reader {
        let _ = reader.join();
    }
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
    let copy = std::io::copy(&mut stdout, &mut archive);
    // Closing stdout is the abort signal: ssh sees EPIPE if the guest is still
    // writing. Do not kill the direct child here, for the timeout reason above.
    drop(stdout);
    let status = child
        .wait()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("ssh failed: {error}")))?;
    let stderr = join_stderr(stderr_reader)?;
    if let Err(error) = &copy {
        let remote_detail = if stderr.is_empty() {
            String::new()
        } else {
            format!("; remote: {}", remote_detail(&stderr))
        };
        return Err(Error::new(
            codes::RUNNER_003,
            format!("cannot read {remote} from qemu: {error}{remote_detail}"),
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
    copy.map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot read {remote} from qemu: {error}"),
        )
    })?;
    archive.seek(SeekFrom::Start(0)).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot rewind the archive from {remote}: {error}"),
        )
    })?;
    tar::Archive::new(archive).unpack(host).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot unpack {remote} into {}: {error}", host.display()),
        )
    })
}

pub(super) fn stream_command_output(
    mut child: Child,
    stdin: Option<&[u8]>,
) -> Result<(ExitStatus, Vec<u8>)> {
    let stdout = take_stdout_echo(&mut child);
    let stderr = take_stderr_with_echo(&mut child);
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
    if let Some(reader) = stdout {
        let _ = reader.join();
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
    fn step_stderr_is_captured_for_the_error_message() {
        let child = piped("printf 'warning\\n' >&2");
        let (status, stderr) = stream_command_output(child, None).expect("stream output");
        assert!(status.success());
        assert_eq!(stderr, b"warning\n");
    }

    #[test]
    fn step_stdin_is_forwarded() {
        let child = piped("wc -c >/dev/null");
        let payload = vec![b'x'; 256 * 1024];
        let (status, _) = stream_command_output(child, Some(&payload)).expect("stream stdin");
        assert!(status.success());
    }
}
