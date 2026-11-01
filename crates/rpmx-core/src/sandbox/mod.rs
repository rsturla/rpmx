use std::collections::BTreeMap;
use std::path::Path;

use crate::arch::Arch;
use crate::error::{codes, Error, Result};
use crate::plan::PlannedStep;

/// Deterministic environment for every step. The host environment is never
/// inherited: it can contain credentials and it differs between machines.
const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Variables that define the build contract. A recipe may set toolchain
/// variables, but it may not move the staging tree or rewrite the timestamp.
const RESERVED_ENV: &[&str] = &["SOURCE_DATE_EPOCH", "SRC", "BUILD", "STAGE", "JOBS"];

mod bwrap;
mod podman;
mod process;
mod qemu;

pub use bwrap::BwrapRunner;
pub use podman::PodmanRunner;
pub use qemu::QemuRunner;

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
