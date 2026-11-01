use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::arch::Arch;
use crate::error::{codes, Error, Result};
use crate::runner::RunnerConfig;

/// The guest serial console is diagnostic only; cap what it can write to the
/// host so a noisy guest cannot fill the disk.
const LOG_LIMIT: usize = 1024 * 1024;

struct CappedLog {
    file: std::fs::File,
    written: usize,
    limit: usize,
}

impl CappedLog {
    fn new(file: std::fs::File, limit: usize) -> Self {
        Self {
            file,
            written: 0,
            limit,
        }
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.written >= self.limit {
            return Ok(());
        }
        let take = bytes.len().min(self.limit - self.written);
        self.file.write_all(&bytes[..take])?;
        self.written += take;
        Ok(())
    }
}

fn pump_log<R: Read + Send + 'static>(mut stream: R, log: Arc<Mutex<CappedLog>>) {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if let Ok(mut log) = log.lock() {
                        let _ = log.write(&chunk[..read]);
                    }
                }
            }
        }
    });
}

pub(super) fn qemu_binary(target: Arch) -> Result<String> {
    if let Ok(value) = std::env::var("RPMX_QEMU_SYSTEM") {
        return Ok(value);
    }
    match target {
        Arch::X86_64 => Ok("qemu-system-x86_64".to_string()),
        Arch::Aarch64 => Ok("qemu-system-aarch64".to_string()),
        other => Err(Error::new(
            codes::RUNNER_002,
            format!("no qemu-system backend for {other}"),
        )),
    }
}

pub(super) fn command_exists(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).exists();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).exists()))
}

pub(super) fn find_kernel(root: &Path) -> Result<PathBuf> {
    let boot = root.join("boot");
    if let Ok(entries) = std::fs::read_dir(&boot) {
        if let Some(path) = entries.flatten().map(|entry| entry.path()).find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("vmlinuz-"))
        }) {
            return Ok(path);
        }
    }
    let modules = root.join("usr/lib/modules");
    if let Ok(entries) = std::fs::read_dir(&modules) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("vmlinuz");
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    Err(Error::new(
        codes::RUNNER_003,
        format!(
            "qemu-system runner needs a kernel in {}/boot or {}/usr/lib/modules/*/vmlinuz",
            root.display(),
            root.display()
        ),
    ))
}

pub(super) fn unpack_kernel(kernel: &Path, scratch: &Path) -> Result<PathBuf> {
    let mut header = [0u8; 32];
    let mut file = std::fs::File::open(kernel).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot read kernel {}: {error}", kernel.display()),
        )
    })?;
    let read = file.read(&mut header).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot read kernel header: {error}"),
        )
    })?;
    if read < 32 || &header[0..2] != b"MZ" || &header[4..8] != b"zimg" {
        return Ok(kernel.to_path_buf());
    }
    let offset = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
    let compression = String::from_utf8_lossy(&header[24..28]);
    if compression != "zstd" {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "cannot unpack EFI zboot kernel {}: unsupported compression {compression}",
                kernel.display()
            ),
        ));
    }
    let bytes = std::fs::read(kernel).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot read kernel {}: {error}", kernel.display()),
        )
    })?;
    let end = offset
        .checked_add(size)
        .ok_or_else(|| Error::new(codes::RUNNER_003, "EFI zboot kernel size overflows"))?;
    if end > bytes.len() {
        return Err(Error::new(
            codes::RUNNER_003,
            "EFI zboot kernel payload extends past the file",
        ));
    }
    let image = zstd::stream::decode_all(&bytes[offset..end]).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot decompress kernel {}: {error}", kernel.display()),
        )
    })?;
    let path = scratch.join("Image");
    std::fs::write(&path, image).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot write {}: {error}", path.display()),
        )
    })?;
    Ok(path)
}

pub(super) fn find_initrd(root: &Path) -> Result<PathBuf> {
    let boot = root.join("boot");
    if let Ok(entries) = std::fs::read_dir(&boot) {
        if let Some(path) = entries.flatten().map(|entry| entry.path()).find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("initramfs-") && name.ends_with(".img"))
        }) {
            return Ok(path);
        }
    }
    Err(Error::new(
        codes::RUNNER_003,
        format!(
            "qemu-system runner needs an initramfs in {}/boot",
            root.display()
        ),
    ))
}

pub(super) fn make_disk(root: &Path, disk: &Path) -> Result<()> {
    let size = directory_size_kb(root)?
        .saturating_mul(2)
        .max(2 * 1024 * 1024);
    let output = Command::new("unshare")
        .args(["-Ur", "mke2fs", "-q", "-t", "ext4", "-d"])
        .arg(root)
        .arg("-F")
        .arg(disk)
        .arg(format!("{size}K"))
        .output()
        .map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot create qemu root disk: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "cannot create qemu root disk: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}

pub(super) fn directory_size_kb(root: &Path) -> Result<u64> {
    let output = Command::new("du")
        .args(["-sk"])
        .arg(root)
        .output()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot size root: {error}")))?;
    if !output.status.success() {
        return Err(Error::new(codes::RUNNER_003, "cannot size root"));
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| Error::new(codes::RUNNER_003, "cannot parse root size"))
}

pub(super) fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot allocate a qemu ssh port: {error}"),
        )
    })?;
    Ok(listener
        .local_addr()
        .map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot read qemu ssh port: {error}"),
            )
        })?
        .port())
}

pub(super) fn start_vm(
    qemu: &str,
    config: &RunnerConfig<'_>,
    kernel: &Path,
    initrd: &Path,
    disk: &Path,
    port: u16,
    log: &Path,
) -> Result<std::process::Child> {
    let mut args = vec![
        "-M".to_string(),
        if config.target == Arch::X86_64 {
            "q35".to_string()
        } else {
            "virt".to_string()
        },
        "-m".to_string(),
        "2048".to_string(),
        "-smp".to_string(),
        config.jobs.to_string(),
        "-no-reboot".to_string(),
        "-nographic".to_string(),
        "-kernel".to_string(),
        kernel.display().to_string(),
        "-initrd".to_string(),
        initrd.display().to_string(),
        "-append".to_string(),
        "console=ttyS0 root=/dev/vda rw".to_string(),
        "-drive".to_string(),
        format!("file={},format=raw,if=virtio", disk.display()),
        "-netdev".to_string(),
        format!("user,id=net0,restrict=on,hostfwd=tcp:127.0.0.1:{port}-:22"),
        "-device".to_string(),
        "virtio-net-pci,netdev=net0,romfile=".to_string(),
    ];
    if config.host == config.target && Path::new("/dev/kvm").exists() {
        args.push("-accel".to_string());
        args.push("kvm".to_string());
        args.push("-cpu".to_string());
        args.push("host".to_string());
    } else {
        args.push("-accel".to_string());
        args.push("tcg".to_string());
        args.push("-cpu".to_string());
        args.push("max".to_string());
    }
    let log_file = std::fs::File::create(log).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot create {}: {error}", log.display()),
        )
    })?;
    let log = Arc::new(Mutex::new(CappedLog::new(log_file, LOG_LIMIT)));
    let mut child = Command::new(qemu)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot start {qemu}: {error}")))?;
    if let Some(stdout) = child.stdout.take() {
        pump_log(stdout, Arc::clone(&log));
    }
    if let Some(stderr) = child.stderr.take() {
        pump_log(stderr, log);
    }
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::CappedLog;

    #[test]
    fn capped_log_discards_bytes_after_the_limit() {
        let dir = tempfile::tempdir().expect("work");
        let path = dir.path().join("log");
        let file = std::fs::File::create(&path).expect("log");
        let mut log = CappedLog::new(file, 8);
        log.write(b"123456").expect("first write");
        log.write(b"789012").expect("second write");
        drop(log);
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "12345678");
    }
}
