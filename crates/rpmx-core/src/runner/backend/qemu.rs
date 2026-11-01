use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::arch::Arch;
use crate::error::{codes, Error, Result};
use crate::runner::{Runner, RunnerConfig, RunnerRequest, Session};

pub struct QemuBackend<'a> {
    pub root: &'a Path,
}

impl Runner for QemuBackend<'_> {
    fn name(&self) -> &'static str {
        "qemu-system"
    }

    fn supports(&self, _host: Arch, target: Arch) -> bool {
        qemu_binary(target)
            .map(|path| command_exists(&path))
            .unwrap_or(false)
    }

    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>> {
        QemuSession::prepare(self.root, config)
    }
}

struct QemuSession {
    workspace: PathBuf,
    stage: PathBuf,
    temp: tempfile::TempDir,
    child: std::process::Child,
    ssh_key: PathBuf,
    port: u16,
    log: PathBuf,
    timeout: Option<u64>,
}

impl QemuSession {
    fn prepare(root: &Path, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>> {
        let qemu = qemu_binary(config.target)?;
        if !command_exists(&qemu) {
            return Err(Error::new(
                codes::RUNNER_002,
                format!("qemu-system runner requires {qemu} on PATH"),
            ));
        }
        let temp = tempfile::tempdir().map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot create qemu scratch directory: {error}"),
            )
        })?;
        let kernel = unpack_kernel(&find_kernel(root)?, temp.path())?;
        let initrd = find_initrd(root)?;
        let disk = temp.path().join("root.img");
        make_disk(root, &disk)?;

        let ssh_key = temp.path().join("id_ed25519");
        generate_ssh_key(&ssh_key)?;
        inject_ssh(root, &disk, &ssh_key.with_extension("pub"), temp.path())?;
        inject_network(root, &disk, temp.path())?;

        let port = free_port()?;
        let log = temp.path().join("qemu.log");
        let child = start_vm(&qemu, config, &kernel, &initrd, &disk, port, &log)?;
        let mut session = Self {
            workspace: config.workspace.to_path_buf(),
            stage: config.stage.to_path_buf(),
            temp,
            child,
            ssh_key,
            port,
            log,
            timeout: config.timeout,
        };
        session.wait_for_ssh()?;
        session.copy_in(config.workspace, crate::guest::BUILD)?;
        session.copy_in(config.stage, crate::guest::STAGE)?;
        Ok(Box::new(session))
    }

    fn ssh_args(&self) -> Vec<String> {
        vec![
            "-T".to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=no".to_string(),
            "-o".to_string(),
            "UserKnownHostsFile=/dev/null".to_string(),
            "-o".to_string(),
            "LogLevel=ERROR".to_string(),
            "-o".to_string(),
            "ConnectTimeout=5".to_string(),
            "-i".to_string(),
            self.ssh_key.display().to_string(),
            "-p".to_string(),
            self.port.to_string(),
            "root@127.0.0.1".to_string(),
        ]
    }

    fn ssh_command(&self, command: &[&str]) -> Command {
        let mut process = match self.timeout {
            Some(seconds) if seconds > 0 => {
                let mut process = Command::new("timeout");
                process
                    .arg("--signal=TERM")
                    .arg("--kill-after=5")
                    .arg(seconds.to_string())
                    .arg("ssh");
                process
            }
            _ => Command::new("ssh"),
        };
        process.args(self.ssh_args()).args(command);
        process
    }

    fn ssh_child(&self, command: &[&str]) -> Result<Child> {
        let mut process = self.ssh_command(command);
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process
            .spawn()
            .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot run ssh: {error}")))
    }

    fn take_stderr(child: &mut Child) -> Option<JoinHandle<std::io::Result<Vec<u8>>>> {
        child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut buffer = Vec::new();
                stderr.read_to_end(&mut buffer)?;
                Ok(buffer)
            })
        })
    }

    fn join_stderr(handle: Option<JoinHandle<std::io::Result<Vec<u8>>>>) -> Result<Vec<u8>> {
        let Some(handle) = handle else {
            return Ok(Vec::new());
        };
        let buffer = handle
            .join()
            .map_err(|_| Error::new(codes::RUNNER_003, "ssh stderr reader panicked"))?
            .map_err(|error| {
                Error::new(
                    codes::RUNNER_003,
                    format!("cannot read ssh stderr: {error}"),
                )
            })?;
        Ok(buffer)
    }

    fn ssh(&self, command: &[&str], stdin: Option<&[u8]>) -> Result<std::process::Output> {
        let mut process = self.ssh_command(command);
        process.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        process.stdout(Stdio::piped());
        process.stderr(Stdio::piped());
        let mut child = process
            .spawn()
            .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot run ssh: {error}")))?;
        if let Some(bytes) = stdin {
            if let Some(mut input) = child.stdin.take() {
                input.write_all(bytes).map_err(|error| {
                    Error::new(codes::RUNNER_003, format!("cannot write to ssh: {error}"))
                })?;
            }
        }
        child
            .wait_with_output()
            .map_err(|error| Error::new(codes::RUNNER_003, format!("ssh failed: {error}")))
    }

    fn wait_for_ssh(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Some(status) = self.child.try_wait().map_err(|error| {
                Error::new(codes::RUNNER_003, format!("cannot poll qemu: {error}"))
            })? {
                let log = std::fs::read_to_string(&self.log).unwrap_or_default();
                return Err(Error::new(
                    codes::RUNNER_003,
                    format!(
                        "qemu exited before ssh was ready ({status}):\n{}",
                        log.lines()
                            .rev()
                            .take(12)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                ));
            }
            let probe = self.ssh(&["true"], None);
            if let Ok(output) = probe {
                if output.status.success() {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                let log = std::fs::read_to_string(&self.log).unwrap_or_default();
                return Err(Error::new(
                    codes::RUNNER_003,
                    format!(
                        "qemu ssh did not become ready within 180s:\n{}",
                        log.lines()
                            .rev()
                            .take(12)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn copy_in(&self, source: &Path, destination: &str) -> Result<()> {
        let command = format!("mkdir -p {destination} && tar -C {destination} -xf -");
        stream_tar_in(self.ssh_child(&[command.as_str()])?, source)
    }

    fn sync_path(&self, remote: &str, host: &Path) -> Result<()> {
        std::fs::create_dir_all(host).map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot create {}: {error}", host.display()),
            )
        })?;
        let archive = tempfile::tempfile_in(host).map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!(
                    "cannot create a temporary archive in {}: {error}",
                    host.display()
                ),
            )
        })?;
        let command = format!("tar -C {remote} -cf - .");
        stream_tar_out(self.ssh_child(&[command.as_str()])?, remote, host, archive)
    }
}

fn stream_tar_in(mut child: Child, source: &Path) -> Result<()> {
    let stderr_reader = QemuSession::take_stderr(&mut child);
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
    drop(stdin);
    if archive.is_err() {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("ssh failed: {error}")))?;
    let stderr = QemuSession::join_stderr(stderr_reader)?;
    if let Some(reader) = stdout_reader {
        let _ = reader.join();
    }
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr);
        let suffix = match &archive {
            Ok(()) => String::new(),
            Err(error) => format!("; archive error: {error}"),
        };
        return Err(Error::new(
            codes::RUNNER_003,
            format!("cannot copy workspace into qemu: {}{suffix}", detail.trim()),
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

fn stream_tar_out(
    mut child: Child,
    remote: &str,
    host: &Path,
    mut archive: std::fs::File,
) -> Result<()> {
    let stderr_reader = QemuSession::take_stderr(&mut child);
    let mut stdout = child.stdout.take().ok_or_else(|| {
        Error::new(
            codes::RUNNER_003,
            "ssh stdout is not available for sync-out",
        )
    })?;
    let copy = std::io::copy(&mut stdout, &mut archive);
    drop(stdout);
    if copy.is_err() {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("ssh failed: {error}")))?;
    let stderr = QemuSession::join_stderr(stderr_reader)?;
    copy.map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot read {remote} from qemu: {error}"),
        )
    })?;
    if !status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "cannot read {remote} from qemu: {}",
                String::from_utf8_lossy(&stderr).trim()
            ),
        ));
    }
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

impl Session for QemuSession {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()> {
        let script = step_script(request);
        let output = self.ssh(&["sh", "-s"], Some(script.as_bytes()))?;
        let _ = std::io::stdout().write_all(&output.stdout);
        let _ = std::io::stderr().write_all(&output.stderr);
        if !output.status.success() {
            return Err(Error::new(
                codes::SANDBOX_002,
                format!(
                    "qemu step '{}' ({}) failed with {}: {}",
                    request.label,
                    request.command,
                    output
                        .status
                        .code()
                        .map_or_else(|| "a signal".to_string(), |code| format!("exit {code}")),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        Ok(())
    }

    fn sync_out(&mut self) -> Result<()> {
        self.sync_path(crate::guest::BUILD, &self.workspace)?;
        self.sync_path(crate::guest::STAGE, &self.stage)
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Drop for QemuSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(self.temp.path());
    }
}

fn generate_ssh_key(key: &Path) -> Result<()> {
    let output = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(key)
        .output()
        .map_err(|error| {
            Error::new(codes::RUNNER_003, format!("cannot run ssh-keygen: {error}"))
        })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "ssh-keygen failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}

fn inject_ssh(root: &Path, disk: &Path, public_key: &Path, scratch: &Path) -> Result<()> {
    if !root.join("usr/sbin/sshd").exists() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "the qemu root must include openssh-server (missing {})",
                root.join("usr/sbin/sshd").display()
            ),
        ));
    }
    let sshd_config = scratch.join("sshd_config");
    std::fs::write(&sshd_config, SSHD_CONFIG).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot write sshd config: {error}"),
        )
    })?;
    let host_key = scratch.join("ssh_host_ed25519_key");
    let output = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&host_key)
        .output()
        .map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot generate ssh host key: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "ssh host key generation failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    let _ = debugfs_write(disk, "mkdir /root/.ssh");
    debugfs_write(
        disk,
        &format!("write {} /etc/ssh/sshd_config", sshd_config.display()),
    )?;
    debugfs_write(disk, "sif /etc/ssh/sshd_config mode 0100644")?;
    debugfs_write(disk, "sif /etc/ssh/sshd_config uid 0")?;
    debugfs_write(disk, "sif /etc/ssh/sshd_config gid 0")?;
    debugfs_write(
        disk,
        &format!("write {} /root/.ssh/authorized_keys", public_key.display()),
    )?;
    debugfs_write(disk, "sif /root/.ssh/authorized_keys mode 0100600")?;
    debugfs_write(disk, "sif /root/.ssh/authorized_keys uid 0")?;
    debugfs_write(disk, "sif /root/.ssh/authorized_keys gid 0")?;
    debugfs_write(disk, "sif /root/.ssh mode 040700")?;
    debugfs_write(disk, "sif /root/.ssh uid 0")?;
    debugfs_write(disk, "sif /root/.ssh gid 0")?;
    debugfs_write(
        disk,
        &format!("write {} /etc/ssh/ssh_host_ed25519_key", host_key.display()),
    )?;
    debugfs_write(disk, "sif /etc/ssh/ssh_host_ed25519_key mode 0100600")?;
    debugfs_write(disk, "sif /etc/ssh/ssh_host_ed25519_key uid 0")?;
    debugfs_write(disk, "sif /etc/ssh/ssh_host_ed25519_key gid 0")?;
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system");
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system/multi-user.target.wants");
    let _ = debugfs_write(
        disk,
        "symlink /etc/systemd/system/multi-user.target.wants/sshd.service /usr/lib/systemd/system/sshd.service",
    );
    Ok(())
}

fn inject_network(root: &Path, disk: &Path, scratch: &Path) -> Result<()> {
    let unit = root.join("usr/lib/systemd/system/systemd-networkd.service");
    if !unit.exists() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "the qemu root must include systemd-networkd (missing {})",
                unit.display()
            ),
        ));
    }
    let config = scratch.join("20-rpmx.network");
    std::fs::write(&config, "[Match]\nType=ether\n\n[Network]\nDHCP=yes\n").map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot write {}: {error}", config.display()),
        )
    })?;
    let _ = debugfs_write(disk, "mkdir /etc/systemd/network");
    debugfs_write(
        disk,
        &format!(
            "write {} /etc/systemd/network/20-rpmx.network",
            config.display()
        ),
    )?;
    debugfs_write(
        disk,
        "sif /etc/systemd/network/20-rpmx.network mode 0100644",
    )?;
    debugfs_write(disk, "sif /etc/systemd/network/20-rpmx.network uid 0")?;
    debugfs_write(disk, "sif /etc/systemd/network/20-rpmx.network gid 0")?;
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system");
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system/multi-user.target.wants");
    let _ = debugfs_write(
        disk,
        "symlink /etc/systemd/system/multi-user.target.wants/systemd-networkd.service /usr/lib/systemd/system/systemd-networkd.service",
    );
    Ok(())
}

fn debugfs_write(disk: &Path, command: &str) -> Result<()> {
    let output = Command::new("debugfs")
        .arg("-w")
        .arg("-R")
        .arg(command)
        .arg(disk)
        .output()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot run debugfs: {error}")))?;
    if !output.status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "debugfs '{}' failed: {}",
                command,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}

fn qemu_binary(target: Arch) -> Result<String> {
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

fn command_exists(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).exists();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).exists()))
}

fn find_kernel(root: &Path) -> Result<PathBuf> {
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

fn unpack_kernel(kernel: &Path, scratch: &Path) -> Result<PathBuf> {
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

fn find_initrd(root: &Path) -> Result<PathBuf> {
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

fn make_disk(root: &Path, disk: &Path) -> Result<()> {
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

fn directory_size_kb(root: &Path) -> Result<u64> {
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

fn free_port() -> Result<u16> {
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

fn start_vm(
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
        config.jobs.max(1).to_string(),
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
    let stderr = std::fs::File::create(log).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot create {}: {error}", log.display()),
        )
    })?;
    let stdout = stderr
        .try_clone()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot clone log: {error}")))?;
    Command::new(qemu)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot start {qemu}: {error}")))
}

const SSHD_CONFIG: &str = "Port 22\n\
PermitRootLogin prohibit-password\n\
PasswordAuthentication no\n\
PubkeyAuthentication yes\n\
UsePAM no\n\
HostKey /etc/ssh/ssh_host_ed25519_key\n\
Subsystem sftp internal-sftp\n";

fn step_script(request: &RunnerRequest<'_>) -> String {
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

fn shell_quote(value: &str) -> String {
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
    use super::{shell_quote, stream_tar_in, stream_tar_out};
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
    fn streamed_copy_in_preserves_the_workspace() {
        let root = tempfile::tempdir().expect("work");
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        std::fs::create_dir_all(source.join("nested")).expect("source dir");
        std::fs::write(source.join("nested/payload.bin"), vec![0xab; 1024 * 1024])
            .expect("payload");

        let quoted = shell_quote(&destination.display().to_string());
        let child = piped(&format!("mkdir -p {quoted} && tar -C {quoted} -xf -"));
        stream_tar_in(child, &source).expect("stream copy-in");

        let copied = std::fs::read(destination.join("nested/payload.bin")).expect("copied payload");
        assert_eq!(copied.len(), 1024 * 1024);
        assert!(copied.iter().all(|byte| *byte == 0xab));
    }

    #[test]
    fn streamed_sync_out_preserves_the_workspace() {
        let root = tempfile::tempdir().expect("work");
        let remote = root.path().join("remote");
        let host = root.path().join("host");
        std::fs::create_dir_all(remote.join("nested")).expect("remote dir");
        std::fs::write(remote.join("nested/payload.txt"), b"streamed\n").expect("payload");
        std::fs::create_dir_all(&host).expect("host dir");

        let quoted = shell_quote(&remote.display().to_string());
        let child = piped(&format!("tar -C {quoted} -cf - ."));
        let archive = tempfile::tempfile_in(&host).expect("archive");
        stream_tar_out(child, "remote", &host, archive).expect("stream sync-out");

        assert_eq!(
            std::fs::read(host.join("nested/payload.txt")).expect("copied payload"),
            b"streamed\n"
        );
    }

    #[test]
    fn streamed_copy_in_reports_remote_failure() {
        let root = tempfile::tempdir().expect("work");
        let source = root.path().join("source");
        std::fs::create_dir_all(&source).expect("source dir");
        std::fs::write(source.join("file.txt"), b"x").expect("payload");

        let child = piped("cat >/dev/null; echo boom >&2; exit 1");
        let error = stream_tar_in(child, &source).expect_err("remote failure");
        assert!(error.message.contains("boom"), "{}", error.message);
    }

    #[test]
    fn streamed_sync_out_reports_remote_failure() {
        let root = tempfile::tempdir().expect("work");
        let host = root.path().join("host");
        std::fs::create_dir_all(&host).expect("host dir");

        let child = piped("echo boom >&2; exit 1");
        let archive = tempfile::tempfile_in(&host).expect("archive");
        let error = stream_tar_out(child, "remote", &host, archive).expect_err("remote failure");
        assert!(error.message.contains("boom"), "{}", error.message);
    }
    #[test]
    fn failed_sync_out_does_not_modify_the_host() {
        let root = tempfile::tempdir().expect("work");
        let host = root.path().join("host");
        std::fs::create_dir_all(&host).expect("host dir");
        std::fs::write(host.join("existing.txt"), b"old").expect("host file");

        let archive_path = root.path().join("payload.tar");
        let file = std::fs::File::create(&archive_path).expect("archive");
        let mut builder = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_path("existing.txt").expect("path");
        header.set_size(3);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"new"[..]).expect("append");
        builder.finish().expect("finish");

        let child = piped(&format!(
            "cat {}; exit 1",
            shell_quote(&archive_path.display().to_string())
        ));
        let archive = tempfile::tempfile_in(&host).expect("archive");
        let error = stream_tar_out(child, "remote", &host, archive).expect_err("remote failure");
        assert_eq!(error.code, "RPMX-RUNNER-003");
        assert_eq!(
            std::fs::read(host.join("existing.txt")).expect("host file"),
            b"old"
        );
    }
}
