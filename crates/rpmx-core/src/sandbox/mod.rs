use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::Path;

use crate::arch::Arch;
use crate::error::{codes, Error, Result};
use crate::plan::PlannedStep;
use crate::runner::{unsupported, Runner, RunnerConfig, RunnerKind, RunnerRequest, Session};

/// Deterministic environment for every step. The host environment is never
/// inherited: it can contain credentials and it differs between machines.
const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Variables that define the build contract. A recipe may set toolchain
/// variables, but it may not move the staging tree or rewrite the timestamp.
const RESERVED_ENV: &[&str] = &["SOURCE_DATE_EPOCH", "SRC", "BUILD", "STAGE", "JOBS"];

/// Everything the sandbox decides before a runner starts: which backend to
/// use, where the build root comes from, and the deterministic policy fields.
#[derive(Clone, Copy)]
pub struct SandboxConfig<'a> {
    pub runner: RunnerKind,
    pub host: Arch,
    pub target: Arch,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    pub root: Option<&'a Path>,
    pub image: Option<&'a str>,
    pub source_date_epoch: u32,
    pub jobs: NonZeroU32,
    pub timeout: Option<NonZeroU64>,
    pub debuginfo: bool,
}

impl<'a> SandboxConfig<'a> {
    fn runner_config(&self) -> RunnerConfig<'a> {
        RunnerConfig {
            host: self.host,
            target: self.target,
            workspace: self.workspace,
            stage: self.stage,
            jobs: self.jobs,
            timeout: self.timeout,
        }
    }
}

pub struct Sandbox<'a> {
    runner: Box<dyn Runner + 'a>,
    config: SandboxConfig<'a>,
    session: std::cell::RefCell<Option<Box<dyn Session>>>,
}

impl<'a> Sandbox<'a> {
    /// Resolve the configured runner kind and prepare a sandbox.
    pub fn new(config: SandboxConfig<'a>) -> Result<Self> {
        if config.root.is_some() && config.image.is_some() {
            return Err(Error::new(
                codes::ROOT_016,
                "a sandbox cannot combine an assembled root with a builder image",
            ));
        }
        let runner =
            config
                .runner
                .resolve(config.host, config.target, config.root, config.image)?;
        Ok(Self::with_runner(config, runner))
    }

    /// Prepare a sandbox around a caller-provided runner. Compatibility with
    /// the configured host/target pair is checked when the session starts, so
    /// constructing a sandbox that is never used cannot fail on the runner.
    pub fn with_runner(config: SandboxConfig<'a>, runner: Box<dyn Runner + 'a>) -> Self {
        Self {
            runner,
            config,
            session: std::cell::RefCell::new(None),
        }
    }

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
            if !self.runner.supports(self.config.host, self.config.target) {
                return Err(unsupported(
                    self.runner.name(),
                    self.config.host,
                    self.config.target,
                ));
            }
            *session = Some(self.runner.start(&self.config.runner_config())?);
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
}

impl Drop for Sandbox<'_> {
    fn drop(&mut self) {
        if let Some(mut session) = self.session.get_mut().take() {
            let _ = session.stop();
        }
    }
}

fn base_env(sandbox: &Sandbox<'_>) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("PATH", SANDBOX_PATH.to_string()),
        ("HOME", crate::guest::BUILD.to_string()),
        ("TMPDIR", "/tmp".to_string()),
        ("LANG", "C.UTF-8".to_string()),
        ("LC_ALL", "C.UTF-8".to_string()),
        ("TZ", "UTC".to_string()),
        (
            "SOURCE_DATE_EPOCH",
            sandbox.config.source_date_epoch.to_string(),
        ),
        ("SRC", crate::guest::BUILD.to_string()),
        ("BUILD", crate::guest::BUILD.to_string()),
        ("STAGE", crate::guest::STAGE.to_string()),
        ("JOBS", sandbox.config.jobs.to_string()),
        ("RPMX_HOST_ARCH", sandbox.config.host.to_string()),
        ("RPMX_TARGET_ARCH", sandbox.config.target.to_string()),
        (
            "RPMX_CROSS",
            if sandbox.config.host == sandbox.config.target {
                "0"
            } else {
                "1"
            }
            .to_string(),
        ),
        (
            "RPMX_DEBUGINFO",
            if sandbox.config.debuginfo { "1" } else { "0" }.to_string(),
        ),
    ];
    if sandbox.config.debuginfo {
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
        .replace("${jobs}", &sandbox.config.jobs.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Sandbox, SandboxConfig};
    use crate::arch::Arch;
    use crate::runner::RunnerKind;
    use std::num::NonZeroU32;
    use std::path::Path;

    #[test]
    fn a_root_and_an_image_cannot_both_be_configured() {
        let config = SandboxConfig {
            runner: RunnerKind::Auto,
            host: Arch::X86_64,
            target: Arch::X86_64,
            workspace: Path::new("/tmp"),
            stage: Path::new("/tmp"),
            root: Some(Path::new("/root")),
            image: Some("image"),
            source_date_epoch: 1,
            jobs: NonZeroU32::new(1).expect("jobs"),
            timeout: None,
            debuginfo: false,
        };
        let error = match Sandbox::new(config) {
            Ok(_) => panic!("root and image must be mutually exclusive"),
            Err(error) => error,
        };
        assert_eq!(error.code, "RPMX-ROOT-016");
    }
}
