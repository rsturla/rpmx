use std::io::Write;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::runner::{Runner, RunnerAssetKind, RunnerConfig, RunnerRequest, Session};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};

mod disk;
mod guest;
mod ssh;

use disk::{
    command_exists, find_initrd, find_kernel, free_port, make_disk, qemu_binary, start_vm,
    unpack_kernel,
};
static QEMU_START_LOCK: Mutex<()> = Mutex::new(());

use guest::{
    generate_ssh_key, inject_build_mount, inject_network, inject_ssh, known_hosts, APPLIANCE_LABEL,
    BUILD_LABEL, BUILD_ROOT_MOUNT,
};
use ssh::{
    shell_quote, step_command, step_script, stream_command_output, stream_tar_in, stream_tar_out,
};

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
        QemuSession::prepare(self.root, config)
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
}

impl QemuSession {
    fn prepare(root: &Path, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>> {
        let appliance = config
            .assets
            .get(RunnerAssetKind::QemuAppliance)
            .ok_or_else(|| {
                Error::new(
                    codes::RUNNER_004,
                    "the qemu runner needs an assembled transport appliance; resolve and assemble one first",
                )
            })?;
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
        let kernel = unpack_kernel(&find_kernel(appliance)?, temp.path())?;
        let initrd = find_initrd(appliance)?;
        let appliance_disk = temp.path().join("appliance.img");
        make_disk(appliance, &appliance_disk, APPLIANCE_LABEL)?;
        let build_disk = temp.path().join("root.img");
        make_disk(root, &build_disk, BUILD_LABEL)?;

        let ssh_key = temp.path().join("id_ed25519");
        generate_ssh_key(&ssh_key)?;
        let host_key = temp.path().join("ssh_host_ed25519_key");
        generate_ssh_key(&host_key)?;
        inject_ssh(
            appliance,
            &appliance_disk,
            &ssh_key.with_extension("pub"),
            &host_key,
            temp.path(),
        )?;
        inject_network(appliance, &appliance_disk, temp.path())?;
        inject_build_mount(&appliance_disk, temp.path())?;

        // Port allocation is a probe-then-bind window. Serialize VM startup
        // within this process so two architecture workers cannot choose the
        // same forwarding port.
        let _starting = QEMU_START_LOCK
            .lock()
            .map_err(|_| Error::new(codes::RUNNER_003, "qemu start lock is poisoned"))?;
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
        };
        session.wait_for_ssh()?;
        session.copy_in(config.workspace, &Self::guest_build_path(), "workspace")?;
        session.copy_in(config.stage, &Self::guest_stage_path(), "stage")?;
        Ok(Box::new(session))
    }

    fn ssh_args(&self, port: u16) -> Vec<String> {
        ssh_args_for(&self.known_hosts, &self.ssh_key, port)
    }

    fn ssh_command_with_timeout(
        &self,
        port: u16,
        command: &[&str],
        timeout: NonZeroU64,
    ) -> Command {
        let mut process = Command::new("timeout");
        process
            .arg("--signal=TERM")
            .arg("--kill-after=5")
            .arg(timeout.get().to_string())
            .arg("ssh");
        process.args(self.ssh_args(port)).args(command);
        process
    }

    fn ssh_command(&self, port: u16, command: &[&str]) -> Command {
        self.ssh_command_with_timeout(port, command, self.timeout)
    }

    fn ssh_child(&self, port: u16, command: &[&str]) -> Result<Child> {
        let mut process = self.ssh_command(port, command);
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process
            .spawn()
            .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot run ssh: {error}")))
    }

    fn ssh_run_with_timeout(
        &self,
        port: u16,
        command: &[&str],
        stdin: Option<&[u8]>,
        timeout: NonZeroU64,
    ) -> Result<(std::process::ExitStatus, Vec<u8>)> {
        let mut process = self.ssh_command_with_timeout(port, command, timeout);
        process.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        process.stdout(Stdio::piped());
        process.stderr(Stdio::piped());
        let child = process
            .spawn()
            .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot run ssh: {error}")))?;
        stream_command_output(child, stdin)
    }

    fn wait_for_ssh(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(180);
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
                self.ssh_run_with_timeout(self.control_port, &["true"], None, probe_timeout)
            {
                if status.success()
                    && self
                        .ssh_run_with_timeout(self.build_port, &["true"], None, probe_timeout)
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
        format!("{BUILD_ROOT_MOUNT}{}", rpmx_base::guest::BUILD)
    }

    fn guest_stage_path() -> String {
        format!("{BUILD_ROOT_MOUNT}{}", rpmx_base::guest::STAGE)
    }

    fn copy_in(&self, source: &Path, destination: &str, label: &str) -> Result<()> {
        let destination = shell_quote(destination);
        let command = format!(
            "for p in /run/rpmx/root/tmp /run/rpmx/root/tmp/build /run/rpmx/root/tmp/stage; do \
             [ -L \"$p\" ] && exit 1; done; mkdir -p {destination} && tar -C {destination} -xf -"
        );
        stream_tar_in(
            self.ssh_child(self.control_port, &[command.as_str()])?,
            source,
            label,
        )
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
        stream_tar_out(
            self.ssh_child(self.control_port, &[command.as_str()])?,
            remote,
            host,
            archive,
        )
    }
}

// SSH startup is part of the host elapsed time, but not the guest deadline.
// A late ordinary failure must keep its command failure classification.
fn step_timed_out(
    status: std::process::ExitStatus,
    elapsed: Duration,
    timeout: NonZeroU64,
) -> bool {
    matches!(status.code(), Some(124 | 137)) && elapsed >= Duration::from_secs(timeout.get())
}

impl Session for QemuSession {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()> {
        let script = step_script(request);
        // Give the appliance's supervisor time to send KILL and return its
        // status before the host-side transport watchdog closes SSH.
        let transport_timeout = NonZeroU64::new(request.timeout.get().saturating_add(10))
            .expect("positive transport timeout");
        let command = step_command(request.timeout);
        let started = Instant::now();
        let (status, stderr) = self.ssh_run_with_timeout(
            self.control_port,
            &[&command],
            Some(script.as_bytes()),
            transport_timeout,
        )?;
        if !status.success() {
            if step_timed_out(status, started.elapsed(), request.timeout) {
                return Err(Error::new(
                    codes::SANDBOX_004,
                    format!(
                        "qemu step '{}' timed out after {}s",
                        request.label,
                        request.timeout.get()
                    ),
                ));
            }
            return Err(Error::new(
                codes::SANDBOX_002,
                format!(
                    "qemu step '{}' ({}) failed with {}: {}",
                    request.label,
                    request.command,
                    status
                        .code()
                        .map_or_else(|| "a signal".to_string(), |code| format!("exit {code}")),
                    String::from_utf8_lossy(&stderr).trim()
                ),
            ));
        }
        Ok(())
    }

    fn sync_out(&mut self) -> Result<()> {
        self.sync_path(&Self::guest_build_path(), &self.workspace)?;
        self.sync_path(&Self::guest_stage_path(), &self.stage)
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
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
            NonZeroU64::new(1).unwrap()
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
            NonZeroU64::new(1).unwrap()
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
