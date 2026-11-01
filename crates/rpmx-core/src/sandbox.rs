use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::arch::Arch;
use crate::error::{codes, Error, Result};
use crate::plan::PlannedStep;

/// Deterministic environment for every step. The host environment is never
/// inherited: it can contain credentials and it differs between machines.
const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Variables that define the build contract. A recipe may set toolchain
/// variables, but it may not move the staging tree or rewrite the timestamp.
const RESERVED_ENV: &[&str] = &["SOURCE_DATE_EPOCH", "SRC", "BUILD", "STAGE", "JOBS"];

static CID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A prepared execution environment. Sessions own their resources, so a QEMU
/// session can keep its root disk between steps while a bubblewrap session is
/// stateless.
pub struct SessionConfig<'a> {
    pub host: Arch,
    pub target: Arch,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    pub source_date_epoch: u32,
    pub jobs: u32,
    pub timeout: Option<u64>,
    pub debuginfo: bool,
}

pub struct RunnerRequest<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub cwd: &'a str,
    pub env: &'a BTreeMap<String, String>,
    pub label: &'a str,
}

/// Whether the host has an enabled qemu-user binfmt handler for `target`.
/// Root assembly still uses this; build steps use the QEMU system runner.
pub fn emulation_available(target: Arch) -> bool {
    let qemu = match target {
        Arch::I686 => "i386",
        other => other.as_str(),
    };
    let path = format!("/proc/sys/fs/binfmt_misc/qemu-{qemu}");
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let enabled = contents.lines().any(|line| line.trim() == "enabled");
    let interpreter = contents
        .lines()
        .find_map(|line| line.strip_prefix("interpreter "))
        .map(Path::new);
    enabled && interpreter.is_some_and(Path::exists)
}

pub trait Session {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()>;
    /// Copy host-visible outputs back from the session. This is a no-op for
    /// bind-mounted runners and a workspace transfer for the QEMU VM.
    fn sync_out(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}

/// A way to create an execution environment.
pub trait Runner {
    fn name(&self) -> &'static str;
    fn supports(&self, host: Arch, target: Arch) -> bool;
    fn start(&self, config: &SessionConfig<'_>) -> Result<Box<dyn Session>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerKind {
    Auto,
    Bwrap,
    Qemu,
}

impl std::str::FromStr for RunnerKind {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "bwrap" => Ok(Self::Bwrap),
            "qemu" => Ok(Self::Qemu),
            other => Err(Error::new(
                codes::RUNNER_001,
                format!("unknown runner '{other}'; expected auto, bwrap or qemu"),
            )),
        }
    }
}

#[derive(Clone, Copy)]
pub enum SandboxBackend<'a> {
    /// Use the host root read-only (prototype).
    Bwrap,
    /// Use an assembled root directory read-only.
    NativeRoot(&'a Path),
    /// Use an assembled root inside a full QEMU VM.
    Qemu(&'a Path),
    /// Use a builder image.
    Container(&'a str),
    /// Use a caller-provided runner.
    Custom(&'a dyn Runner),
}

impl<'a> RunnerKind {
    /// Turn the command-line runner choice and the available roots into one
    /// backend. Every caller must use this, so `auto` cannot drift between
    /// source assembly, rebuild and installed tests.
    pub fn select(
        self,
        host: Arch,
        target: Arch,
        root: Option<&'a Path>,
        image: Option<&'a str>,
    ) -> Result<SandboxBackend<'a>> {
        match self {
            Self::Qemu => {
                let root = root.ok_or_else(|| {
                    Error::new(codes::ROOT_016, "the qemu runner needs an assembled root")
                })?;
                Ok(SandboxBackend::Qemu(root))
            }
            Self::Bwrap => match root {
                Some(_root) if host != target => Err(Error::new(
                    codes::RUNNER_002,
                    "bubblewrap cannot run a foreign target; use --runner qemu",
                )),
                Some(root) => Ok(SandboxBackend::NativeRoot(root)),
                None => Ok(SandboxBackend::Bwrap),
            },
            Self::Auto => match root {
                Some(root) if host == target => Ok(SandboxBackend::NativeRoot(root)),
                Some(root) => Ok(SandboxBackend::Qemu(root)),
                None => match image {
                    Some(image) => Ok(SandboxBackend::Container(image)),
                    None => Ok(SandboxBackend::Bwrap),
                },
            },
        }
    }
}

pub struct Sandbox<'a> {
    pub backend: SandboxBackend<'a>,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    pub host_arch: Arch,
    pub target_arch: Arch,
    pub source_date_epoch: u32,
    pub jobs: u32,
    pub timeout: Option<u64>,
    pub debuginfo: bool,
    pub session: std::cell::RefCell<Option<Box<dyn Session>>>,
}

impl<'a> Sandbox<'a> {
    pub fn run(&self, step: &PlannedStep) -> Result<()> {
        for key in step.env.keys() {
            if RESERVED_ENV.contains(&key.as_str()) {
                return Err(Error::new(
                    codes::SANDBOX_003,
                    format!(
                        "step '{}' cannot set reserved sandbox variable '{key}'",
                        step.uses
                    ),
                ));
            }
        }

        let command = step.command.as_deref().ok_or_else(|| {
            Error::new(
                codes::BUILD_004,
                format!(
                    "built-in step '{}' has no executable command yet",
                    step.uses
                ),
            )
        })?;
        let args = step
            .args
            .iter()
            .map(|arg| expand(arg, self))
            .collect::<Vec<_>>();
        let cwd = step
            .cwd
            .as_deref()
            .map(|cwd| expand(cwd, self))
            .unwrap_or_else(|| crate::guest::BUILD.to_string());

        let mut env = BTreeMap::new();
        for (key, value) in base_env(self) {
            env.insert(key.to_string(), value);
        }
        for (key, value) in &step.env {
            env.insert(key.clone(), expand(value, self));
        }

        let request = RunnerRequest {
            command,
            args: &args,
            cwd: &cwd,
            env: &env,
            label: &step.uses,
        };

        let mut session = self.session.borrow_mut();
        if session.is_none() {
            *session = Some(self.start_session()?);
        }
        session
            .as_mut()
            .expect("session was initialized")
            .run(&request)
    }

    /// Flush session-local outputs to the host workspace and stage.
    pub fn sync(&self) -> Result<()> {
        if let Some(session) = self.session.borrow_mut().as_mut() {
            session.sync_out()?;
        }
        Ok(())
    }

    fn start_session(&self) -> Result<Box<dyn Session>> {
        let config = SessionConfig {
            host: self.host_arch,
            target: self.target_arch,
            workspace: self.workspace,
            stage: self.stage,
            source_date_epoch: self.source_date_epoch,
            jobs: self.jobs,
            timeout: self.timeout,
            debuginfo: self.debuginfo,
        };
        let runner: Box<dyn Runner> = match self.backend {
            SandboxBackend::Bwrap => Box::new(BwrapRunner { root: None }),
            SandboxBackend::NativeRoot(root) => {
                if self.host_arch == self.target_arch {
                    Box::new(BwrapRunner { root: Some(root) })
                } else {
                    Box::new(QemuRunner { root })
                }
            }
            SandboxBackend::Qemu(root) => Box::new(QemuRunner { root }),
            SandboxBackend::Container(image) => Box::new(PodmanRunner { image }),
            SandboxBackend::Custom(runner) => {
                if !runner.supports(self.host_arch, self.target_arch) {
                    return Err(unsupported(runner.name(), self.host_arch, self.target_arch));
                }
                return runner.start(&config);
            }
        };
        if !runner.supports(self.host_arch, self.target_arch) {
            return Err(unsupported(runner.name(), self.host_arch, self.target_arch));
        }
        runner.start(&config)
    }
}

impl Drop for Sandbox<'_> {
    fn drop(&mut self) {
        if let Some(mut session) = self.session.get_mut().take() {
            let _ = session.stop();
        }
    }
}

fn unsupported(runner: &str, host: Arch, target: Arch) -> Error {
    Error::new(
        codes::RUNNER_002,
        format!(
            "runner '{runner}' cannot execute {target} steps on a {host} host; \
             use a runner that supports this architecture"
        ),
    )
}

// ---------------------------------------------------------------------------
// bubblewrap

pub struct BwrapRunner<'a> {
    pub root: Option<&'a Path>,
}

impl Runner for BwrapRunner<'_> {
    fn name(&self) -> &'static str {
        "bubblewrap"
    }

    fn supports(&self, host: Arch, target: Arch) -> bool {
        host == target
    }

    fn start(&self, config: &SessionConfig<'_>) -> Result<Box<dyn Session>> {
        Ok(Box::new(BwrapSession {
            root: self
                .root
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/")),
            workspace: config.workspace.to_path_buf(),
            stage: config.stage.to_path_buf(),
            timeout: config.timeout,
        }))
    }
}

struct BwrapSession {
    root: PathBuf,
    workspace: PathBuf,
    stage: PathBuf,
    timeout: Option<u64>,
}

impl Session for BwrapSession {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()> {
        let mut args = vec![
            "--unshare-all".to_string(),
            "--unshare-user".to_string(),
            "--disable-userns".to_string(),
            "--die-with-parent".to_string(),
            "--new-session".to_string(),
            "--clearenv".to_string(),
            "--ro-bind".to_string(),
            self.root.display().to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--tmpfs".to_string(),
            "/sys".to_string(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
            "--bind".to_string(),
            self.workspace.display().to_string(),
            crate::guest::BUILD.to_string(),
            "--bind".to_string(),
            self.stage.display().to_string(),
            crate::guest::STAGE.to_string(),
            "--chdir".to_string(),
            request.cwd.to_string(),
        ];
        for (key, value) in request.env {
            args.push("--setenv".to_string());
            args.push(key.clone());
            args.push(value.clone());
        }
        args.push("--".to_string());
        args.push(request.command.to_string());
        args.extend(request.args.iter().cloned());

        let completion = spawn("bwrap", &args, self.timeout, request.label)?;
        finish(completion, self.timeout, request.label, "")
    }

    fn sync_out(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// podman

pub struct PodmanRunner<'a> {
    pub image: &'a str,
}

impl Runner for PodmanRunner<'_> {
    fn name(&self) -> &'static str {
        "podman"
    }

    fn supports(&self, host: Arch, target: Arch) -> bool {
        host == target
    }

    fn start(&self, config: &SessionConfig<'_>) -> Result<Box<dyn Session>> {
        let name = format!(
            "rpmx-run-{}-{}",
            std::process::id(),
            CID_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            name.clone(),
            "--arch".to_string(),
            config.target.to_string(),
            "--network=none".to_string(),
            "--read-only".to_string(),
            "--cap-drop=all".to_string(),
            "--security-opt=no-new-privileges".to_string(),
            "--pids-limit=1024".to_string(),
            "--userns=keep-id".to_string(),
            "--tmpfs".to_string(),
            "/tmp:rw,size=2g".to_string(),
            "-v".to_string(),
            format!("{}:/tmp/build:Z", config.workspace.display()),
            "-v".to_string(),
            format!("{}:/tmp/stage:Z", config.stage.display()),
            "--entrypoint".to_string(),
            "/bin/sh".to_string(),
            self.image.to_string(),
            "-c".to_string(),
            "trap : TERM INT; sleep infinity & wait".to_string(),
        ];
        let output = Command::new("podman")
            .args(&args)
            .output()
            .map_err(|error| {
                Error::new(codes::SANDBOX_001, format!("cannot start podman: {error}"))
            })?;
        if !output.status.success() {
            return Err(Error::new(
                codes::SANDBOX_002,
                format!(
                    "cannot start podman session: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        Ok(Box::new(PodmanSession {
            name,
            timeout: config.timeout,
        }))
    }
}

struct PodmanSession {
    name: String,
    timeout: Option<u64>,
}

impl Session for PodmanSession {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()> {
        let mut args = vec![
            "exec".to_string(),
            "-i".to_string(),
            "-w".to_string(),
            request.cwd.to_string(),
        ];
        for (key, value) in request.env {
            args.push("-e".to_string());
            args.push(format!("{key}={value}"));
        }
        args.push(self.name.clone());
        args.push(request.command.to_string());
        args.extend(request.args.iter().cloned());

        let completion = spawn("podman", &args, self.timeout, request.label)?;
        if timed_out(&completion, self.timeout) {
            let _ = Command::new("podman")
                .args(["rm", "-f", &self.name])
                .status();
        }
        finish(completion, self.timeout, request.label, "container ")
    }

    fn sync_out(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        let _ = Command::new("podman")
            .args(["rm", "-f", &self.name])
            .status();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// qemu-system

pub struct QemuRunner<'a> {
    pub root: &'a Path,
}

impl Runner for QemuRunner<'_> {
    fn name(&self) -> &'static str {
        "qemu-system"
    }

    fn supports(&self, _host: Arch, target: Arch) -> bool {
        qemu_binary(target)
            .map(|path| command_exists(&path))
            .unwrap_or(false)
    }

    fn start(&self, config: &SessionConfig<'_>) -> Result<Box<dyn Session>> {
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
    fn prepare(root: &Path, config: &SessionConfig<'_>) -> Result<Box<dyn Session>> {
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

    fn ssh(&self, command: &[&str], stdin: Option<&[u8]>) -> Result<std::process::Output> {
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
        process.stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
        process.stdout(std::process::Stdio::piped());
        process.stderr(std::process::Stdio::piped());
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
        let mut archive = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut archive);
            builder.follow_symlinks(false);
            builder.append_dir_all(".", source).map_err(|error| {
                Error::new(
                    codes::RUNNER_003,
                    format!("cannot archive {}: {error}", source.display()),
                )
            })?;
            builder.finish().map_err(|error| {
                Error::new(codes::RUNNER_003, format!("cannot finish archive: {error}"))
            })?;
        }
        let command = format!("mkdir -p {destination} && tar -C {destination} -xf -");
        let output = self.ssh(&[command.as_str()], Some(&archive))?;
        if !output.status.success() {
            return Err(Error::new(
                codes::RUNNER_003,
                format!(
                    "cannot copy workspace into qemu: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        Ok(())
    }

    fn sync_path(&self, remote: &str, host: &Path) -> Result<()> {
        std::fs::create_dir_all(host).map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot create {}: {error}", host.display()),
            )
        })?;
        let command = format!("tar -C {remote} -cf - .");
        let output = self.ssh(&[command.as_str()], None)?;
        if !output.status.success() {
            return Err(Error::new(
                codes::RUNNER_003,
                format!(
                    "cannot read {remote} from qemu: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        let mut archive = tar::Archive::new(std::io::Cursor::new(output.stdout));
        archive.unpack(host).map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot unpack {remote} into {}: {error}", host.display()),
            )
        })
    }
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
    use std::io::Read;
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
    config: &SessionConfig<'_>,
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

// ---------------------------------------------------------------------------
// helpers

struct Completion {
    status: ExitStatus,
    elapsed: Duration,
}

fn spawn(program: &str, args: &[String], timeout: Option<u64>, label: &str) -> Result<Completion> {
    let mut process = match timeout {
        Some(seconds) if seconds > 0 => {
            let mut process = Command::new("timeout");
            process
                .arg("--signal=TERM")
                .arg("--kill-after=5")
                .arg(seconds.to_string())
                .arg(program);
            process
        }
        _ => Command::new(program),
    };
    process.args(args);
    let started = Instant::now();
    let status = process.status().map_err(|error| {
        Error::new(
            codes::SANDBOX_001,
            format!("cannot start {program} for '{label}': {error}"),
        )
    })?;
    Ok(Completion {
        status,
        elapsed: started.elapsed(),
    })
}

fn timed_out(completion: &Completion, timeout: Option<u64>) -> bool {
    timeout.is_some_and(|seconds| seconds > 0)
        && completion.elapsed >= Duration::from_secs(timeout.unwrap_or(0))
        && !completion.status.success()
}

fn finish(completion: Completion, timeout: Option<u64>, label: &str, prefix: &str) -> Result<()> {
    if timed_out(&completion, timeout) {
        return Err(Error::new(
            codes::SANDBOX_004,
            format!(
                "{prefix}step '{label}' timed out after {}s",
                timeout.unwrap_or(0)
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
                    .map_or_else(|| "a signal".to_string(), |code| format!("exit {code}"))
            ),
        ));
    }
    Ok(())
}

fn base_env(sandbox: &Sandbox<'_>) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("PATH", SANDBOX_PATH.to_string()),
        ("HOME", crate::guest::BUILD.to_string()),
        ("TMPDIR", "/tmp".to_string()),
        ("LANG", "C.UTF-8".to_string()),
        ("LC_ALL", "C.UTF-8".to_string()),
        ("TZ", "UTC".to_string()),
        ("SOURCE_DATE_EPOCH", sandbox.source_date_epoch.to_string()),
        ("SRC", crate::guest::BUILD.to_string()),
        ("BUILD", crate::guest::BUILD.to_string()),
        ("STAGE", crate::guest::STAGE.to_string()),
        ("JOBS", sandbox.jobs.to_string()),
        ("RPMX_HOST_ARCH", sandbox.host_arch.to_string()),
        ("RPMX_TARGET_ARCH", sandbox.target_arch.to_string()),
        (
            "RPMX_CROSS",
            if sandbox.host_arch == sandbox.target_arch {
                "0"
            } else {
                "1"
            }
            .to_string(),
        ),
        (
            "RPMX_DEBUGINFO",
            if sandbox.debuginfo { "1" } else { "0" }.to_string(),
        ),
    ];
    if sandbox.debuginfo {
        env.push(("CFLAGS", "-g".to_string()));
        env.push(("CXXFLAGS", "-g".to_string()));
    }
    env
}

fn expand(value: &str, sandbox: &Sandbox<'_>) -> String {
    value
        .replace("${source.dir}", crate::guest::BUILD)
        .replace("${build.dir}", crate::guest::BUILD)
        .replace("${stage}", crate::guest::STAGE)
        .replace("${prefix}", crate::guest::PREFIX)
        .replace("${libdir}", crate::guest::LIBDIR)
        .replace("${jobs}", &sandbox.jobs.to_string())
}

use std::io::Write;
