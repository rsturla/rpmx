use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::arch::Arch;
use crate::error::{codes, Error, Result};
use crate::runner::{Runner, RunnerConfig, RunnerRequest, Session};

mod disk;
mod guest;
mod ssh;

use disk::{
    command_exists, find_initrd, find_kernel, free_port, make_disk, qemu_binary, start_vm,
    unpack_kernel,
};
use guest::{generate_ssh_key, inject_network, inject_ssh};
use ssh::{shell_quote, step_script, stream_command_output, stream_tar_in, stream_tar_out};

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
    timeout: Option<NonZeroU64>,
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
        session.copy_in(config.workspace, crate::guest::BUILD, "workspace")?;
        session.copy_in(config.stage, crate::guest::STAGE, "stage")?;
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
            Some(seconds) => {
                let mut process = Command::new("timeout");
                process
                    .arg("--signal=TERM")
                    .arg("--kill-after=5")
                    .arg(seconds.get().to_string())
                    .arg("ssh");
                process
            }
            None => Command::new("ssh"),
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

    /// Run a command over ssh, streaming stdout and stderr to the host while
    /// retaining a bounded stderr tail for the step's error message.
    fn ssh_run(
        &self,
        command: &[&str],
        stdin: Option<&[u8]>,
    ) -> Result<(std::process::ExitStatus, Vec<u8>)> {
        let mut process = self.ssh_command(command);
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
            if let Ok((status, _)) = self.ssh_run(&["true"], None) {
                if status.success() {
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

    fn copy_in(&self, source: &Path, destination: &str, label: &str) -> Result<()> {
        let destination = shell_quote(destination);
        let command = format!("mkdir -p {destination} && tar -C {destination} -xf -");
        stream_tar_in(self.ssh_child(&[command.as_str()])?, source, label)
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
        stream_tar_out(self.ssh_child(&[command.as_str()])?, remote, host, archive)
    }
}

impl Session for QemuSession {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()> {
        let script = step_script(request);
        let (status, stderr) = self.ssh_run(&["sh", "-s"], Some(script.as_bytes()))?;
        if !status.success() {
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

#[cfg(test)]
mod tests {
    use super::ssh::{shell_quote, stream_tar_in, stream_tar_out};
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

        // tee echoes the tar stream to stdout while tar extracts it. A buffered
        // copy-in would fill the stdout pipe and deadlock; timeout bounds the
        // test if that regression returns.
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
            error.message.contains("cannot read remote from qemu"),
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
            error.message.contains("cannot read remote"),
            "{}",
            error.message
        );
        assert!(error.message.contains("warning"), "{}", error.message);
    }
}
