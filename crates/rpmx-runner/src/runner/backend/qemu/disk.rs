use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use super::guest::{BUILD_PORT, CONTROL_PORT};
use crate::runner::RunnerConfig;
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};

/// The guest serial console is diagnostic only; cap what it can write to the
/// host so a noisy guest cannot fill the disk.
const LOG_LIMIT: usize = 1024 * 1024;

/// A decompressed kernel image larger than this is not legitimate.
const MAX_KERNEL_BYTES: u64 = 512 * 1024 * 1024;

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
    unpack_kernel_with_limit(kernel, scratch, MAX_KERNEL_BYTES)
}

fn unpack_kernel_with_limit(kernel: &Path, scratch: &Path, limit: u64) -> Result<PathBuf> {
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
    let offset = u64::from(u32::from_le_bytes(header[8..12].try_into().unwrap()));
    let size = u64::from(u32::from_le_bytes(header[12..16].try_into().unwrap()));
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
    let end = offset
        .checked_add(size)
        .ok_or_else(|| Error::new(codes::RUNNER_003, "EFI zboot kernel size overflows"))?;
    let length = file
        .metadata()
        .map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot stat kernel {}: {error}", kernel.display()),
            )
        })?
        .len();
    if end > length {
        return Err(Error::new(
            codes::RUNNER_003,
            "EFI zboot kernel payload extends past the file",
        ));
    }
    file.seek(SeekFrom::Start(offset)).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot seek kernel {}: {error}", kernel.display()),
        )
    })?;
    let decoder = zstd::stream::read::Decoder::new(file.take(size)).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!(
                "cannot open compressed kernel {}: {error}",
                kernel.display()
            ),
        )
    })?;
    let path = scratch.join("Image");
    let mut output = std::fs::File::create(&path).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot write {}: {error}", path.display()),
        )
    })?;
    let copied = std::io::copy(&mut decoder.take(limit + 1), &mut output).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot decompress kernel {}: {error}", kernel.display()),
        )
    })?;
    if copied > limit {
        drop(output);
        let _ = std::fs::remove_file(&path);
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "decompressed kernel {} exceeds the {limit}-byte limit",
                kernel.display()
            ),
        ));
    }
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

pub(super) fn make_disk(root: &Path, disk: &Path, label: &str) -> Result<()> {
    let size = directory_size_kb(root)?
        .saturating_mul(2)
        .max(2 * 1024 * 1024);
    let output = Command::new("unshare")
        .args(["-Ur", "mke2fs", "-q", "-t", "ext4", "-L", label, "-d"])
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
    appliance_disk: &Path,
    build_disk: &Path,
    control_port: u16,
    build_port: u16,
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
        "console=ttyS0 root=LABEL=rpmx-appliance rw".to_string(),
        "-drive".to_string(),
        format!("file={},format=raw,if=virtio", appliance_disk.display()),
        "-drive".to_string(),
        format!("file={},format=raw,if=virtio", build_disk.display()),
        "-netdev".to_string(),
        format!(
            "user,id=net0,restrict=on,hostfwd=tcp:127.0.0.1:{control_port}-:{CONTROL_PORT},hostfwd=tcp:127.0.0.1:{build_port}-:{BUILD_PORT}"
        ),
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
    fn efi_zboot_kernel_is_streamed_and_unpacked() {
        use std::io::Write;

        let dir = tempfile::tempdir().expect("work");
        let kernel = dir.path().join("vmlinuz");
        let payload = zstd::stream::encode_all(&b"kernel image"[..], 0).expect("compress");
        let mut header = [0u8; 32];
        header[0..2].copy_from_slice(b"MZ");
        header[4..8].copy_from_slice(b"zimg");
        header[8..12].copy_from_slice(&32u32.to_le_bytes());
        header[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        header[24..28].copy_from_slice(b"zstd");
        let mut file = std::fs::File::create(&kernel).expect("kernel");
        file.write_all(&header).expect("header");
        file.write_all(&payload).expect("payload");

        let image = super::unpack_kernel(&kernel, dir.path()).expect("unpack kernel");
        assert_eq!(std::fs::read(image).expect("image"), b"kernel image");
    }

    #[test]
    fn oversized_efi_zboot_kernel_is_rejected_and_removed() {
        use std::io::Write;

        let dir = tempfile::tempdir().expect("work");
        let kernel = dir.path().join("vmlinuz");
        let payload = zstd::stream::encode_all(&b"kernel image"[..], 0).expect("compress");
        let mut header = [0u8; 32];
        header[0..2].copy_from_slice(b"MZ");
        header[4..8].copy_from_slice(b"zimg");
        header[8..12].copy_from_slice(&32u32.to_le_bytes());
        header[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        header[24..28].copy_from_slice(b"zstd");
        let mut file = std::fs::File::create(&kernel).expect("kernel");
        file.write_all(&header).expect("header");
        file.write_all(&payload).expect("payload");

        let error =
            super::unpack_kernel_with_limit(&kernel, dir.path(), 4).expect_err("kernel cap");
        assert!(error.message.contains("exceeds"), "{}", error.message);
        assert!(!dir.path().join("Image").exists());
    }

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
