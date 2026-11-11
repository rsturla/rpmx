use std::io::Write;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::runner::{Runner, RunnerAssetKind, RunnerConfig, RunnerRequest, Session};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};

mod budget;
mod disk;
mod guest;
mod policy;
mod ssh;

use budget::{seconds, Budget};
use disk::{
    command_exists, find_initrd, find_kernel, free_port, make_disk, qemu_binary, start_vm,
    unpack_kernel,
};
use policy::{PayloadPolicy, PAYLOAD_SLICE};
static QEMU_START_LOCK: Mutex<()> = Mutex::new(());

use guest::{
    generate_ssh_key, inject_build_mount, inject_network, inject_ssh, known_hosts, APPLIANCE_LABEL,
    BUILD_LABEL,
};
use ssh::{shell_quote, step_script, stream_command_output, stream_tar_in, stream_tar_out};

pub struct QemuBackend<'a> {
    pub root: &'a Path,
}

impl Runner for QemuBackend<'_> {
    fn name(&self) -> &'static str {
        "qemu-system"
    }

    fn supports(&self, _host: Arch, exec: Arch) -> bool {
        qemu_binary(exec)
            .map(|path| command_exists(&path))
            .unwrap_or(false)
    }

    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>> {
        Ok(Box::new(QemuSession::prepare(self.root, config)?))
    }
}

struct QemuSession {
    workspace: PathBuf,
    stage: PathBuf,
    temp: tempfile::TempDir,
    child: std::process::Child,
    ssh_key: PathBuf,
    host_key: PathBuf,
    known_hosts: PathBuf,
    control_port: u16,
    build_port: u16,
    log: PathBuf,
    timeout: NonZeroU64,
    budget: Budget,
    policy: PayloadPolicy,
    next_step: u64,
    stopped: bool,
}

impl QemuSession {
    fn prepare(root: &Path, config: &RunnerConfig<'_>) -> Result<Self> {
        let budget = Budget {
            deadline: config.policy.deadline,
            default: Duration::from_secs(config.timeout.get()),
        };
        budget.remaining(budget.default, "starting qemu")?;
        for mount in config.policy.readonly_mounts {
            policy::input_target(mount.target)?;
        }
        let appliance = config
            .assets
            .get(RunnerAssetKind::QemuAppliance)
            .ok_or_else(|| {
                Error::new(
                    codes::RUNNER_004,
                    "the qemu runner needs an assembled transport appliance; resolve and assemble one first",
                )
            })?;
        let qemu = qemu_binary(config.exec)?;
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
        let kernel = unpack_kernel(&find_kernel(appliance)?, temp.path())?;
        let initrd = find_initrd(appliance)?;
        let appliance_disk = temp.path().join("appliance.img");
        make_disk(appliance, &appliance_disk, APPLIANCE_LABEL, budget)?;
        let build_disk = temp.path().join("root.img");
        make_disk(root, &build_disk, BUILD_LABEL, budget)?;

        let ssh_key = temp.path().join("id_ed25519");
        generate_ssh_key(&ssh_key, budget)?;
        let host_key = temp.path().join("ssh_host_ed25519_key");
        generate_ssh_key(&host_key, budget)?;
        inject_ssh(
            appliance,
            &appliance_disk,
            &ssh_key.with_extension("pub"),
            &host_key,
            temp.path(),
            budget,
        )?;
        inject_network(appliance, &appliance_disk, temp.path(), budget)?;
        inject_build_mount(&appliance_disk, temp.path(), budget)?;

        // Port allocation is a probe-then-bind window. Serialize VM startup
        // within this process so two architecture workers cannot choose the
        // same forwarding port.
        let _starting = loop {
            match QEMU_START_LOCK.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(Error::new(codes::RUNNER_003, "qemu start lock is poisoned"))
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    budget.remaining(budget.default, "waiting for qemu startup")?;
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        let control_port = free_port()?;
        let build_port = free_port()?;
        let known_hosts = known_hosts(
            temp.path(),
            &host_key.with_extension("pub"),
            control_port,
            build_port,
        )?;
        let log = temp.path().join("qemu.log");
        let child = start_vm(
            &qemu,
            config,
            &kernel,
            &initrd,
            &appliance_disk,
            &build_disk,
            control_port,
            build_port,
            &log,
        )?;
        let mut session = Self {
            workspace: config.workspace.to_path_buf(),
            stage: config.stage.to_path_buf(),
            temp,
            child,
            ssh_key,
            host_key,
            known_hosts,
            control_port,
            build_port,
            log,
            timeout: config.timeout,
            budget,
            policy: PayloadPolicy {
                network: config.policy.network,
                readonly_stage: config.policy.readonly_stage,
                writable_root: config.writable_root,
                mounts: Vec::new(),
            },
            next_step: 0,
            stopped: false,
        };
        session.wait_for_ssh()?;
        session.copy_in(config.workspace, &Self::guest_build_path(), "workspace")?;
        session.copy_in(config.stage, &Self::guest_stage_path(), "stage")?;
        session.control(
            "systemctl start dbus.socket; test -S /run/dbus/system_bus_socket",
            "starting qemu supervision bus",
        )?;
        session.control("command -v systemd-run >/dev/null && command -v timeout >/dev/null && test -f /sys/fs/cgroup/cgroup.controllers", "checking qemu payload supervisor")?;
        session.control("systemctl start rpmx-payload.slice; mkdir -p /run/rpmx/inputs; printf 'nameserver 10.0.2.3\n' >/run/rpmx/resolv.conf; chown -R 65534:65534 /run/rpmx/workspace", "preparing qemu workspace identity")?;
        if !config.policy.readonly_stage {
            session.control(
                "chown -R 65534:65534 /run/rpmx/stage",
                "preparing qemu stage identity",
            )?;
        }
        for (index, mount) in config.policy.readonly_mounts.iter().enumerate() {
            let source = format!("/run/rpmx/inputs/{index}");
            let source = session.copy_input(mount.source, &source)?;
            session
                .policy
                .mounts
                .push((source, mount.target.to_string()));
        }
        if let Some(sysroot) = config.sysroot {
            let source = "/run/rpmx/sysroot";
            session.copy_in(sysroot, source, "target sysroot")?;
            session
                .policy
                .mounts
                .push((source.to_string(), "/sysroot".to_string()));
        }

        Ok(session)
    }

    fn ssh_args(&self, port: u16) -> Vec<String> {
        ssh_args_for(&self.known_hosts, &self.ssh_key, port)
    }

    fn ssh_command_with_timeout(
        &self,
        port: u16,
        command: &[&str],
        timeout: NonZeroU64,
    ) -> Result<Command> {
        let remaining = self
            .budget
            .remaining(Duration::from_secs(timeout.get()), "qemu SSH transport")?;
        let mut process = Command::new("timeout");
        process
            .arg("--signal=TERM")
            .arg("--kill-after=5")
            .arg(seconds(remaining))
            .arg("ssh");
        process.args(self.ssh_args(port)).args(command);
        Ok(process)
    }

    fn ssh_command(&self, port: u16, command: &[&str]) -> Result<Command> {
        self.ssh_command_with_timeout(port, command, self.timeout)
    }

    fn ssh_child(&self, port: u16, command: &[&str]) -> Result<Child> {
        let mut process = self.ssh_command(port, command)?;
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        super::process::spawn_inheriting_fds(process, &[])
            .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot run ssh: {error}")))
    }

    fn ssh_run_with_timeout(
        &self,
        port: u16,
        command: &[&str],
        stdin: Option<&[u8]>,
        timeout: NonZeroU64,
        logs: Option<&crate::runner::OutputLogs>,
    ) -> Result<(std::process::ExitStatus, Vec<u8>)> {
        let mut process = self.ssh_command_with_timeout(port, command, timeout)?;
        process.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        process.stdout(Stdio::piped());
        process.stderr(Stdio::piped());
        let child = super::process::spawn_inheriting_fds(process, &[])
            .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot run ssh: {error}")))?;
        stream_command_output(child, stdin, logs)
    }

    fn wait_for_ssh(&mut self) -> Result<()> {
        let deadline = Instant::now()
            + self
                .budget
                .remaining(Duration::from_secs(180), "booting qemu")?;
        loop {
            if let Some(status) = self.child.try_wait().map_err(|error| {
                Error::new(codes::RUNNER_003, format!("cannot poll qemu: {error}"))
            })? {
                return Err(Error::new(
                    codes::RUNNER_004,
                    format!(
                        "qemu exited before the appliance was ready ({status}):\n{}",
                        log_tail(&self.log)
                    ),
                ));
            }
            let probe_timeout = NonZeroU64::new(15).expect("non-zero probe timeout");
            if let Ok((status, _)) =
                self.ssh_run_with_timeout(self.control_port, &["true"], None, probe_timeout, None)
            {
                if status.success()
                    && self
                        .ssh_run_with_timeout(self.build_port, &["true"], None, probe_timeout, None)
                        .is_ok_and(|(status, _)| status.success())
                {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::new(
                    codes::RUNNER_004,
                    format!(
                        "qemu appliance did not become ready within 180s:\n{}",
                        log_tail(&self.log)
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn guest_build_path() -> String {
        "/run/rpmx/workspace".to_string()
    }

    fn guest_stage_path() -> String {
        "/run/rpmx/stage".to_string()
    }

    fn copy_in(&self, source: &Path, destination: &str, label: &str) -> Result<()> {
        let destination = shell_quote(destination);
        let command = format!("mkdir -p {destination} && tar -C {destination} -xf -");
        stream_tar_in(
            self.ssh_child(self.control_port, &[command.as_str()])?,
            source,
            label,
        )
    }

    fn copy_input(&self, source: &Path, destination: &str) -> Result<String> {
        if source.is_dir() {
            self.copy_in(source, destination, "read-only input")?;
            return Ok(destination.to_string());
        }
        if source.is_file() {
            let wrapper = tempfile::tempdir_in(self.temp.path())
                .map_err(|error| Error::new(codes::RUNNER_003, error.to_string()))?;
            std::fs::copy(source, wrapper.path().join("payload")).map_err(|error| {
                Error::new(
                    codes::RUNNER_003,
                    format!("cannot snapshot qemu input {}: {error}", source.display()),
                )
            })?;
            self.copy_in(wrapper.path(), destination, "read-only file")?;
            return Ok(format!("{destination}/payload"));
        }
        Err(Error::new(
            codes::RUNNER_004,
            format!(
                "qemu input {} must be a regular file or directory",
                source.display()
            ),
        ))
    }

    fn control(&self, command: &str, label: &str) -> Result<()> {
        let command = format!("set -eu; {command}");
        let (status, stderr) =
            self.ssh_run_with_timeout(self.control_port, &[&command], None, self.timeout, None)?;
        if !status.success() {
            return Err(Error::new(
                codes::RUNNER_004,
                format!("{label} failed: {}", String::from_utf8_lossy(&stderr)),
            ));
        }
        Ok(())
    }

    fn supervisor_diagnostics(&self, unit: &str) -> String {
        let mut command = Command::new("ssh");
        command.args(self.ssh_args(self.control_port)).arg(format!(
            "journalctl --no-pager -n 16 -u {}",
            shell_quote(&format!("{unit}.service"))
        ));
        let budget = Budget {
            default: Duration::from_secs(3),
            ..self.budget
        };
        match budget.output(&mut command, "reading qemu supervisor diagnostics") {
            Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_string(),
            Err(_) => String::new(),
        }
    }

    fn quiesce(&self) -> Result<()> {
        self.control(&format!("set -eu; systemctl stop {PAYLOAD_SLICE}; p=/sys/fs/cgroup/rpmx.slice/{PAYLOAD_SLICE}/cgroup.events; if test -f \"$p\"; then ! grep -q '^populated 1$' \"$p\"; fi"), "stopping qemu payload writers")
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
        // Stage the stream in the destination directory and unpack only after
        // the guest exits successfully, so a failed transfer cannot leave a
        // partial tree behind. This trades memory for disk, not RAM.
        let quoted = shell_quote(remote);
        let command = format!("tar -C {quoted} -cf - .");
        let incoming = tempfile::Builder::new()
            .prefix(".rpmx-qemu-export-")
            .tempdir_in(host.parent().ok_or_else(|| {
                Error::new(codes::RUNNER_003, "qemu export requires an owned directory")
            })?)
            .map_err(|error| Error::new(codes::RUNNER_003, error.to_string()))?;
        stream_tar_out(
            self.ssh_child(self.control_port, &[command.as_str()])?,
            remote,
            incoming.path(),
            archive,
        )?;
        replace_export(incoming.path(), host)
    }
}

/// Publish a complete validated snapshot, including guest deletions. Only owned
/// workspace/stage directories reach this function; an existing symlink is refused.
fn replace_export(incoming: &Path, destination: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(destination)
        .map_err(|error| Error::new(codes::RUNNER_003, error.to_string()))?;
    if !metadata.is_dir()
        || std::fs::canonicalize(destination)
            .map_err(|error| Error::new(codes::RUNNER_003, error.to_string()))?
            == Path::new("/")
    {
        return Err(Error::new(
            codes::RUNNER_003,
            "qemu export requires an owned non-root directory",
        ));
    }
    let backup = tempfile::Builder::new()
        .prefix(".rpmx-qemu-previous-")
        .tempdir_in(destination.parent().unwrap())
        .map_err(|error| Error::new(codes::RUNNER_003, error.to_string()))?;
    let previous = backup.path().join("tree");
    std::fs::rename(destination, &previous).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot retain previous qemu workspace: {error}"),
        )
    })?;
    if let Err(error) = std::fs::rename(incoming, destination) {
        let restored = std::fs::rename(&previous, destination);
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "cannot publish qemu export: {error}; previous workspace restoration: {restored:?}"
            ),
        ));
    }
    writable_directories(&previous)
        .and_then(|()| std::fs::remove_dir_all(&previous))
        .map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot remove previous qemu workspace: {error}"),
            )
        })?;
    Ok(())
}

fn writable_directories(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
        )?;
        for entry in std::fs::read_dir(path)? {
            writable_directories(&entry?.path())?;
        }
    }
    Ok(())
}

// SSH startup is part of the host elapsed time, but not the guest deadline.
// A late ordinary failure must keep its command failure classification.
fn step_timed_out(status: std::process::ExitStatus, elapsed: Duration, timeout: Duration) -> bool {
    matches!(status.code(), Some(124 | 137)) && elapsed >= timeout
}

impl Session for QemuSession {
    fn finish(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        // Cleanup has its own small bound after a case deadline. Payload execution
        // cannot borrow this grace period, and failed quiescence forbids export.
        self.budget.deadline = Some(Instant::now() + Duration::from_secs(15));
        let result = self.sync_out();
        let stopped = self.stop();
        match (result, stopped) {
            (Err(error), _) | (_, Err(error)) => Err(error),
            _ => Ok(()),
        }
    }

    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()> {
        if self.stopped {
            return Err(Error::new(
                codes::RUNNER_004,
                "qemu session is already stopped",
            ));
        }
        let remaining = self
            .budget
            .remaining(Duration::from_secs(request.timeout.get()), request.label)?;
        let script = step_script(request);
        let unit = format!("rpmx-payload-{}", self.next_step);
        self.next_step += 1;
        let command = self.policy.command(remaining, &unit);
        let transport_timeout = NonZeroU64::new(request.timeout.get().saturating_add(5))
            .expect("positive transport timeout");
        let started = Instant::now();
        let (status, stderr) = self.ssh_run_with_timeout(
            self.control_port,
            &[&command],
            Some(script.as_bytes()),
            transport_timeout,
            request.logs,
        )?;
        if !status.success() {
            let diagnostics = self.supervisor_diagnostics(&unit);
            if !diagnostics.is_empty() {
                eprintln!("{diagnostics}");
            }
            if step_timed_out(status, started.elapsed(), remaining) {
                return Err(Error::new(
                    codes::SANDBOX_004,
                    format!("qemu step '{}' exceeded its deadline", request.label),
                ));
            }
            return Err(Error::new(
                codes::SANDBOX_002,
                format!(
                    "qemu step '{}' failed with {:?}: {}",
                    request.label,
                    status.code(),
                    format!(
                        "{}\n{}",
                        String::from_utf8_lossy(&stderr).trim(),
                        diagnostics
                    )
                    .trim()
                ),
            ));
        }
        Ok(())
    }

    fn sync_out(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.quiesce()?;
        self.sync_path(&Self::guest_build_path(), &self.workspace)?;
        if !self.policy.readonly_stage {
            self.sync_path(&Self::guest_stage_path(), &self.stage)?;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        match self.child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => {
                self.child.kill().map_err(|error| {
                    Error::new(codes::RUNNER_003, format!("cannot stop qemu: {error}"))
                })?;
                self.child.wait().map_err(|error| {
                    Error::new(codes::RUNNER_003, format!("cannot reap qemu: {error}"))
                })?;
                Ok(())
            }
            Err(error) => Err(Error::new(
                codes::RUNNER_003,
                format!("cannot inspect qemu: {error}"),
            )),
        }
    }
}

impl Drop for QemuSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        secure_delete(&self.ssh_key);
        secure_delete(&self.host_key);
        let _ = std::fs::remove_file(&self.known_hosts);
        let _ = std::fs::remove_dir_all(self.temp.path());
    }
}

/// Return the last lines of the serial log with terminal control sequences
/// stripped. The log is guest output like any other stream.
fn log_tail(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let mut clean = Vec::new();
    let mut sanitizer = crate::terminal::Sanitizer::default();
    sanitizer.push(&bytes, &mut clean);
    String::from_utf8_lossy(&clean)
        .lines()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

fn ssh_args_for(known_hosts: &Path, ssh_key: &Path, port: u16) -> Vec<String> {
    vec![
        "-T".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=yes".to_string(),
        "-o".to_string(),
        format!("UserKnownHostsFile={}", known_hosts.display()),
        "-o".to_string(),
        "CheckHostIP=no".to_string(),
        "-o".to_string(),
        "LogLevel=ERROR".to_string(),
        "-o".to_string(),
        "ConnectTimeout=5".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
        "-i".to_string(),
        ssh_key.display().to_string(),
        "-p".to_string(),
        port.to_string(),
        "root@127.0.0.1".to_string(),
    ]
}

/// Overwrite and remove a private key file before the scratch directory is
/// deleted. The best effort matters because the temp directory can outlive the
/// process on a crash.
fn secure_delete(path: &Path) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) {
        let zeros = [0u8; 4096];
        let mut remaining = metadata.len();
        while remaining > 0 {
            let take = remaining.min(zeros.len() as u64) as usize;
            if file.write_all(&zeros[..take]).is_err() {
                break;
            }
            remaining -= take as u64;
        }
        let _ = file.sync_all();
    }
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("pub"));
}

#[cfg(test)]
mod tests {
    use super::ssh::{shell_quote, stream_tar_in, stream_tar_out};
    use super::{secure_delete, ssh_args_for, step_timed_out, QemuSession, RunnerConfig};
    use crate::runner::RunnerAssets;
    use rpmx_base::arch::Arch;
    use std::num::{NonZeroU32, NonZeroU64};
    use std::process::{Command, Stdio};

    #[test]
    fn a_delayed_ordinary_failure_is_not_a_step_timeout() {
        let started = std::time::Instant::now();
        let status = Command::new("sh")
            .args(["-c", "sleep 1.1; exit 7"])
            .status()
            .unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_secs(1));
        assert!(!step_timed_out(
            status,
            started.elapsed(),
            std::time::Duration::from_secs(1)
        ));
    }

    #[test]
    fn supervisor_deadline_status_is_classified_as_timeout() {
        let started = std::time::Instant::now();
        let status = Command::new("timeout")
            .args(["1", "sleep", "30"])
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(124));
        assert!(step_timed_out(
            status,
            started.elapsed(),
            std::time::Duration::from_secs(1)
        ));
    }

    #[test]
    fn ssh_uses_the_allocated_host_port() {
        let args = ssh_args_for(
            std::path::Path::new("/scratch/known_hosts"),
            std::path::Path::new("/scratch/id_ed25519"),
            41234,
        );
        let port = args
            .iter()
            .position(|argument| argument == "-p")
            .expect("port flag");
        assert_eq!(args[port + 1], "41234");
        assert!(args.contains(&"StrictHostKeyChecking=yes".to_string()));
        assert!(args.contains(&"UserKnownHostsFile=/scratch/known_hosts".to_string()));
    }

    #[test]
    fn a_missing_appliance_is_a_runner_error() {
        let config = RunnerConfig {
            policy: Default::default(),
            writable_root: false,
            host: Arch::X86_64,
            exec: Arch::X86_64,
            target: Arch::X86_64,
            sysroot: None,
            workspace: std::path::Path::new("/tmp"),
            stage: std::path::Path::new("/tmp"),
            assets: RunnerAssets::default(),
            jobs: NonZeroU32::new(1).expect("jobs"),
            timeout: NonZeroU64::new(1).expect("timeout"),
        };
        let error = match QemuSession::prepare(std::path::Path::new("/nonexistent"), &config) {
            Ok(_) => panic!("a missing appliance must fail"),
            Err(error) => error,
        };
        assert_eq!(error.code, "RPMX-RUNNER-004");
        assert!(error.message.contains("appliance"));
    }

    #[test]
    fn secure_delete_removes_a_private_key() {
        use std::io::{Read, Seek, SeekFrom};

        let directory = tempfile::tempdir().expect("tempdir");
        let key = directory.path().join("id_ed25519");
        std::fs::write(&key, b"private-key-material").expect("key");
        let mut handle = std::fs::File::open(&key).expect("open");
        secure_delete(&key);
        assert!(!key.exists());
        handle.seek(SeekFrom::Start(0)).expect("seek");
        let mut contents = Vec::new();
        handle.read_to_end(&mut contents).expect("read");
        assert_eq!(contents.len(), b"private-key-material".len());
        assert!(contents.iter().all(|byte| *byte == 0));
    }

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
        stream_tar_in(child, &source, "workspace").expect("stream copy-in");

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
    fn validated_export_replaces_deleted_files_and_refuses_symlink_destinations() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remote");
        let host = root.path().join("host");
        std::fs::create_dir(&remote).unwrap();
        std::fs::create_dir(&host).unwrap();
        std::fs::write(host.join("deleted-by-guest"), "stale").unwrap();
        std::fs::write(remote.join("result"), "fresh").unwrap();
        let incoming = tempfile::tempdir_in(root.path()).unwrap();
        let child = piped(&format!(
            "tar -C {} -cf - .",
            shell_quote(remote.to_str().unwrap())
        ));
        stream_tar_out(
            child,
            "remote",
            incoming.path(),
            tempfile::tempfile().unwrap(),
        )
        .unwrap();
        super::replace_export(incoming.path(), &host).unwrap();
        assert!(!host.join("deleted-by-guest").exists());
        assert_eq!(
            std::fs::read_to_string(host.join("result")).unwrap(),
            "fresh"
        );
        let symlink = root.path().join("alias");
        std::os::unix::fs::symlink(&host, &symlink).unwrap();
        let incoming = tempfile::tempdir_in(root.path()).unwrap();
        assert!(super::replace_export(incoming.path(), &symlink).is_err());
        assert_eq!(
            std::fs::read_to_string(host.join("result")).unwrap(),
            "fresh"
        );
    }

    #[test]
    fn streamed_copy_in_reports_remote_failure() {
        let root = tempfile::tempdir().expect("work");
        let source = root.path().join("source");
        std::fs::create_dir_all(&source).expect("source dir");
        std::fs::write(source.join("file.txt"), b"x").expect("payload");

        let child = piped("cat >/dev/null; echo boom >&2; exit 1");
        let error = stream_tar_in(child, &source, "workspace").expect_err("remote failure");
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
    fn streamed_copy_in_drains_remote_stdout() {
        let root = tempfile::tempdir().expect("work");
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        std::fs::create_dir_all(&source).expect("source dir");
        std::fs::write(source.join("payload.bin"), vec![0x5a; 512 * 1024]).expect("payload");

        let child = piped(&format!(
            "timeout 20 sh -c 'mkdir -p {}; p={}/pipe; mkfifo \"$p\"; tar -C {} -xf \"$p\" & tee \"$p\"; wait'",
            destination.display(),
            destination.display(),
            destination.display()
        ));
        stream_tar_in(child, &source, "workspace").expect("stream copy-in");

        assert_eq!(
            std::fs::read(destination.join("payload.bin")).expect("copied payload"),
            vec![0x5a; 512 * 1024]
        );
    }

    #[test]
    fn stderr_diagnostics_are_bounded() {
        let root = tempfile::tempdir().expect("work");
        let source = root.path().join("source");
        std::fs::create_dir_all(&source).expect("source dir");
        std::fs::write(source.join("file.txt"), b"x").expect("payload");

        let child = piped(
            "cat >/dev/null; i=0; while [ $i -lt 2000 ]; do printf '%050d\\n' $i >&2; i=$((i+1)); done; exit 1",
        );
        let error = stream_tar_in(child, &source, "workspace").expect_err("remote failure");
        assert!(
            error.message.len() < 70 * 1024,
            "stderr diagnostics grew to {} bytes",
            error.message.len()
        );
        assert!(
            error.message.contains("1999"),
            "the retained diagnostics must keep the final line: {}",
            error.message
        );
    }

    #[test]
    fn sync_out_reports_a_local_write_failure() {
        let full = std::path::Path::new("/dev/full");
        if !full.exists() {
            eprintln!("/dev/full is not available; skipping local write failure test");
            return;
        }
        let root = tempfile::tempdir().expect("work");
        let host = root.path().join("host");
        std::fs::create_dir_all(&host).expect("host dir");
        let archive = std::fs::OpenOptions::new()
            .write(true)
            .open(full)
            .expect("/dev/full");

        let child = piped("head -c 1048576 /dev/zero");
        let error = stream_tar_out(child, "remote", &host, archive).expect_err("full disk");
        assert!(
            error
                .message
                .contains("cannot write the archive for remote"),
            "{}",
            error.message
        );
        assert!(
            error.message.to_lowercase().contains("space"),
            "{}",
            error.message
        );
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

    #[cfg(target_os = "linux")]
    #[test]
    fn local_write_error_is_reported_with_remote_failure() {
        let root = tempfile::tempdir().expect("work");
        let child = piped("head -c 4096 /dev/zero; echo warning >&2; exit 1");
        let archive = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .expect("/dev/full");
        let error =
            stream_tar_out(child, "remote", root.path(), archive).expect_err("write failure");
        assert!(
            error
                .message
                .contains("cannot write the archive for remote"),
            "{}",
            error.message
        );
        assert!(error.message.contains("warning"), "{}", error.message);
    }
}

#[cfg(test)]
mod policy_controls {
    use super::*;
    use crate::runner::{ExecutionPolicy, ReadOnlyMount, RunnerAssets};
    use std::num::NonZeroU32;

    fn run(session: &mut QemuSession, script: &str, timeout: u64) -> Result<()> {
        session.run(&RunnerRequest {
            command: "/bin/bash",
            args: &["-euc".into(), script.into()],
            cwd: "/tmp/build",
            env: &std::collections::BTreeMap::new(),
            timeout: NonZeroU64::new(timeout).unwrap(),
            label: "qemu policy control",
            logs: None,
        })
    }

    #[test]
    #[ignore = "requires RPMX_QEMU_APPLIANCE pointing to an assembled native appliance, qemu-system, native cc with static libc, and KVM or TCG"]
    fn real_vm_enforces_inputs_identity_network_quiescence_and_case_deadline() {
        use std::os::unix::fs::PermissionsExt;
        let appliance = PathBuf::from(
            std::env::var_os("RPMX_QEMU_APPLIANCE")
                .expect("set RPMX_QEMU_APPLIANCE to the assembled native appliance"),
        );
        let scratch = tempfile::tempdir().unwrap();
        let workspace = scratch.path().join("workspace");
        let stage = scratch.path().join("stage");
        let fixture = scratch.path().join("fixture");
        for directory in [&workspace, &stage, &fixture] {
            std::fs::create_dir(directory).unwrap();
        }
        // Compile a dependency-free native consumer: the isolated network must
        // still support a service and client in the same test invocation.
        let source = scratch.path().join("loopback.c");
        std::fs::write(
            &source,
            r#"
#include <arpa/inet.h>
#include <assert.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>
int main(void) {
    int listener = socket(AF_INET, SOCK_STREAM, 0);
    assert(listener >= 0);
    struct sockaddr_in address = { .sin_family = AF_INET,
        .sin_addr.s_addr = htonl(INADDR_LOOPBACK) };
    assert(bind(listener, (struct sockaddr *)&address, sizeof(address)) == 0);
    assert(listen(listener, 1) == 0);
    socklen_t length = sizeof(address);
    assert(getsockname(listener, (struct sockaddr *)&address, &length) == 0);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        close(listener);
        int client = socket(AF_INET, SOCK_STREAM, 0);
        assert(client >= 0);
        assert(connect(client, (struct sockaddr *)&address, length) == 0);
        assert(write(client, "ping", 4) == 4);
        char reply[4];
        assert(recv(client, reply, 4, MSG_WAITALL) == 4 && memcmp(reply, "pong", 4) == 0);
        close(client);
        _exit(0);
    }
    int connection = accept(listener, 0, 0);
    assert(connection >= 0);
    char request[4];
    assert(recv(connection, request, 4, MSG_WAITALL) == 4 && memcmp(request, "ping", 4) == 0);
    assert(write(connection, "pong", 4) == 4);
    close(connection);
    close(listener);
    int status;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    return 0;
}
"#,
        )
        .unwrap();
        let compile = Command::new("cc")
            .args(["-static", "-Wall", "-Wextra", "-Werror"])
            .arg(&source)
            .arg("-o")
            .arg(workspace.join("loopback"))
            .output()
            .expect("native C compiler with static libc is required for the real VM control");
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        std::fs::write(workspace.join("delete-me"), "stale").unwrap();
        std::fs::write(stage.join("payload"), "original").unwrap();
        std::fs::write(fixture.join("value"), "fixture").unwrap();
        std::fs::set_permissions(
            stage.join("payload"),
            std::fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        std::fs::set_permissions(
            fixture.join("value"),
            std::fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        let mounts = [ReadOnlyMount {
            source: &fixture,
            target: "/tmp/rpmx-inputs/fixture",
        }];
        let arch = Arch::current().unwrap();
        let config = RunnerConfig {
            policy: ExecutionPolicy {
                readonly_stage: true,
                readonly_mounts: &mounts,
                deadline: Some(Instant::now() + Duration::from_secs(180)),
                ..Default::default()
            },
            writable_root: false,
            host: arch,
            exec: arch,
            target: arch,
            sysroot: None,
            workspace: &workspace,
            stage: &stage,
            assets: RunnerAssets::new(Some(&appliance), &[]),
            jobs: NonZeroU32::new(2).unwrap(),
            timeout: NonZeroU64::new(180).unwrap(),
        };
        let mut session = QemuSession::prepare(&appliance, &config).unwrap();
        run(
            &mut session,
            r#"
test "$(id -u)" = 65534
grep -Eq '^CapEff:[[:space:]]+0000000000000000$' /proc/self/status
grep -Eq '^NoNewPrivs:[[:space:]]+1$' /proc/self/status
test "$(wc -l </proc/net/route)" = 1
./loopback
test "$(cat /tmp/rpmx-inputs/fixture/value)" = fixture
if (printf bad >/tmp/rpmx-inputs/fixture/value); then exit 61; fi
if (printf bad >/tmp/stage/payload); then exit 62; fi
rm delete-me
printf initial >late-writer
(sleep 1; printf late >>late-writer) &
printf workspace-ok >result
"#,
            20,
        )
        .unwrap();
        session.finish().unwrap();
        assert!(
            session.child.try_wait().unwrap().is_some(),
            "finish must reap the VM"
        );
        assert!(
            !workspace.join("delete-me").exists(),
            "guest deletions must not survive as stale artifacts"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("result")).unwrap(),
            "workspace-ok"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("late-writer")).unwrap(),
            "initial"
        );
        assert_eq!(
            std::fs::read_to_string(stage.join("payload")).unwrap(),
            "original"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.join("value")).unwrap(),
            "fixture"
        );
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(
            std::fs::read_to_string(workspace.join("late-writer")).unwrap(),
            "initial"
        );

        let transaction_config = RunnerConfig {
            writable_root: true,
            ..config
        };
        let mut transaction = QemuSession::prepare(&appliance, &transaction_config).unwrap();
        run(
            &mut transaction,
            r#"
test "$(id -u)" = 0
printf protected >/tmp/build/protected
chmod 000 /tmp/build/protected
test "$(cat /tmp/build/protected)" = protected
chmod 600 /tmp/build/protected
if (printf bad >/tmp/rpmx-inputs/fixture/value); then exit 63; fi
if (printf bad >/tmp/stage/payload); then exit 64; fi
"#,
            20,
        )
        .unwrap();
        transaction.budget.deadline = Some(Instant::now() + Duration::from_millis(1500));
        run(&mut transaction, "sleep 0.8", 10).unwrap();
        let started = Instant::now();
        let error = run(&mut transaction, "sleep 30", 10).unwrap_err();
        assert_eq!(error.code, codes::SANDBOX_004);
        assert!(started.elapsed() < Duration::from_secs(6));
        transaction.finish().unwrap();
        assert!(transaction.child.try_wait().unwrap().is_some());

        let network_config = RunnerConfig {
            policy: ExecutionPolicy {
                network: true,
                ..config.policy
            },
            ..config
        };
        let mut network = QemuSession::prepare(&appliance, &network_config).unwrap();
        run(&mut network, "test \"$(wc -l </proc/net/route)\" -gt 1; getent ahostsv4 registry.fedoraproject.org >dns-result; test -s dns-result", 20).unwrap();
        network.finish().unwrap();
        assert!(!std::fs::read(workspace.join("dns-result"))
            .unwrap()
            .is_empty());
    }
}
