use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::thread::JoinHandle;

use crate::error::{codes, Error, Result};
use crate::runner::RunnerRequest;

/// Keep error diagnostics useful without letting a hostile or noisy guest
/// grow host memory without bound. The last `STDERR_LIMIT` bytes are retained;
/// everything older is drained and discarded.
const STDERR_LIMIT: usize = 64 * 1024;
/// Hard ceiling for a guest archive staged on the host. This bounds disk and
/// the memory the tar crate may use for extension headers.
const MAX_TRANSFER_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// GNU long-name/link and PAX extension headers are read fully into memory by
/// the tar crate; keep that allocation small.
const MAX_EXTENSION_BYTES: u64 = 1024 * 1024;

fn retain_tail(buffer: &mut Vec<u8>, bytes: &[u8]) {
    buffer.extend_from_slice(bytes);
    if buffer.len() > STDERR_LIMIT {
        let overflow = buffer.len() - STDERR_LIMIT;
        buffer.drain(..overflow);
    }
}

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
                retain_tail(&mut buffer, &chunk[..read]);
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
                retain_tail(&mut buffer, &chunk[..read]);
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

fn read_tar_header(reader: &mut std::fs::File, header: &mut [u8; 512]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < header.len() {
        let read = reader.read(&mut header[filled..])?;
        if read == 0 {
            if filled == 0 {
                return Ok(false);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated tar header",
            ));
        }
        filled += read;
    }
    Ok(true)
}

fn parse_tar_size(field: &[u8], remote: &str) -> Result<u64> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        let mut value = u64::from(field[0] & 0x7f);
        for byte in &field[1..] {
            value = value
                .checked_shl(8)
                .and_then(|value| value.checked_add(u64::from(*byte)))
                .ok_or_else(|| {
                    Error::new(
                        codes::RUNNER_003,
                        format!("archive from {remote} has an overflowing entry size"),
                    )
                })?;
        }
        return Ok(value);
    }
    let text = std::str::from_utf8(field).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("archive from {remote} has a non-UTF-8 entry size: {error}"),
        )
    })?;
    let text = text.trim_matches(|character: char| character == '\0' || character == ' ');
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("archive from {remote} has an invalid entry size: {error}"),
        )
    })
}

fn copy_archive(
    reader: &mut impl Read,
    archive: &mut std::fs::File,
    remote: &str,
    host: &Path,
) -> Result<()> {
    copy_archive_with_limit(reader, archive, remote, host, MAX_TRANSFER_BYTES)
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
                format!("archive from {remote} exceeds the {limit}-byte transfer limit"),
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

fn validate_archive(archive: &mut std::fs::File, remote: &str) -> Result<()> {
    archive.seek(SeekFrom::Start(0)).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot rewind the archive from {remote}: {error}"),
        )
    })?;
    let mut header = [0u8; 512];
    loop {
        if !read_tar_header(archive, &mut header).map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot read the archive from {remote}: {error}"),
            )
        })? {
            break;
        }
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let size = parse_tar_size(&header[124..136], remote)?;
        if size > MAX_TRANSFER_BYTES {
            return Err(Error::new(
                codes::RUNNER_003,
                format!(
                    "archive from {remote} declares a {size}-byte entry; the transfer limit is {MAX_TRANSFER_BYTES} bytes"
                ),
            ));
        }
        let typeflag = header[156];
        if matches!(typeflag, b'L' | b'K' | b'x' | b'g') && size > MAX_EXTENSION_BYTES {
            return Err(Error::new(
                codes::RUNNER_003,
                format!(
                    "archive from {remote} has a {size}-byte tar extension header; the limit is {MAX_EXTENSION_BYTES} bytes"
                ),
            ));
        }
        let skip = size.div_ceil(512).saturating_mul(512);
        let skip = i64::try_from(skip).map_err(|_| {
            Error::new(
                codes::RUNNER_003,
                format!("archive from {remote} has an entry too large to skip"),
            )
        })?;
        archive.seek(SeekFrom::Current(skip)).map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot scan the archive from {remote}: {error}"),
            )
        })?;
    }
    archive.seek(SeekFrom::Start(0)).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot rewind the archive from {remote}: {error}"),
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
    let copy = copy_archive(&mut stdout, &mut archive, remote, host);
    // Closing stdout is the abort signal: ssh sees EPIPE if the guest is still
    // writing. Do not kill the direct child here, for the timeout reason above.
    drop(stdout);
    let status = child
        .wait()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("ssh failed: {error}")))?;
    let stderr = join_stderr(stderr_reader)?;
    if let Err(error) = copy {
        if status.success() || stderr.is_empty() {
            return Err(error);
        }
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
    validate_archive(&mut archive, remote)?;
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
    fn archive_transfer_is_capped() {
        let root = tempfile::tempdir().expect("work");
        let path = root.path().join("archive");
        let mut archive = std::fs::File::create(&path).expect("archive");
        let mut reader = std::io::Cursor::new(vec![0u8; 100]);
        let error =
            super::copy_archive_with_limit(&mut reader, &mut archive, "remote", root.path(), 8)
                .expect_err("transfer cap");
        assert!(
            error.message.contains("transfer limit"),
            "{}",
            error.message
        );
    }

    #[test]
    fn oversized_tar_extension_headers_are_rejected() {
        use std::io::{Seek, SeekFrom, Write};

        let root = tempfile::tempdir().expect("work");
        let path = root.path().join("archive");
        let mut archive = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .expect("archive");
        let mut header = [0u8; 512];
        header[156] = b'L';
        let size = format!("{:011o}\0", 2 * 1024 * 1024);
        header[124..136].copy_from_slice(size.as_bytes());
        archive.write_all(&header).expect("header");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        let error = super::validate_archive(&mut archive, "remote").expect_err("extension cap");
        assert!(
            error.message.contains("extension header"),
            "{}",
            error.message
        );
    }

    #[test]
    fn step_stdin_is_forwarded() {
        let child = piped("wc -c >/dev/null");
        let payload = vec![b'x'; 256 * 1024];
        let (status, _) = stream_command_output(child, Some(&payload)).expect("stream stdin");
        assert!(status.success());
    }
}
