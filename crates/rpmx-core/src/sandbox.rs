use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::error::{codes, Error, Result};
use crate::plan::PlannedStep;

/// Deterministic environment for every step. The host environment is never
/// inherited: it can contain credentials and it differs between machines.
const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Variables that define the build contract. A recipe may set toolchain
/// variables, but it may not move the staging tree or rewrite the timestamp.
const RESERVED_ENV: &[&str] = &["SOURCE_DATE_EPOCH", "SRC", "BUILD", "STAGE", "JOBS"];

static CID_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub enum SandboxBackend<'a> {
    /// Use the host root read-only (prototype).
    Bwrap,
    /// Use an assembled root directory read-only.
    Root(&'a Path),
    /// Use a builder image.
    Container(&'a str),
}

pub struct Sandbox<'a> {
    pub backend: SandboxBackend<'a>,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    pub source_date_epoch: u32,
    pub jobs: u32,
    /// Wall-clock limit for each step in seconds. `None` or `Some(0)` disables
    /// it. A timed-out step is reported as `RPMX-SANDBOX-004`.
    pub timeout: Option<u64>,
    /// Extract debug sections into the stage instead of discarding them.
    pub debuginfo: bool,
}

impl Sandbox<'_> {
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
            .unwrap_or_else(|| "/tmp/build".to_string());

        match self.backend {
            SandboxBackend::Container(image) => {
                self.run_container(image, command, &args, &cwd, &step.env, &step.uses)
            }
            SandboxBackend::Root(root) => {
                self.run_bwrap(Some(root), command, &args, &cwd, &step.env, &step.uses)
            }
            SandboxBackend::Bwrap => {
                self.run_bwrap(None, command, &args, &cwd, &step.env, &step.uses)
            }
        }
    }

    fn run_bwrap(
        &self,
        root: Option<&Path>,
        command: &str,
        command_args: &[String],
        cwd: &str,
        env: &std::collections::BTreeMap<String, String>,
        label: &str,
    ) -> Result<()> {
        let mut args = vec![
            "--unshare-all".to_string(),
            "--unshare-user".to_string(),
            "--disable-userns".to_string(),
            "--die-with-parent".to_string(),
            "--new-session".to_string(),
            "--clearenv".to_string(),
            "--ro-bind".to_string(),
            root.unwrap_or_else(|| Path::new("/")).display().to_string(),
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
            "/tmp/build".to_string(),
            "--bind".to_string(),
            self.stage.display().to_string(),
            "/tmp/stage".to_string(),
            "--chdir".to_string(),
            cwd.to_string(),
        ];

        for (key, value) in base_env(self) {
            args.push("--setenv".to_string());
            args.push(key.to_string());
            args.push(value);
        }
        for (key, value) in env {
            args.push("--setenv".to_string());
            args.push(key.clone());
            args.push(expand(value, self));
        }

        args.push("--".to_string());
        args.push(command.to_string());
        args.extend(command_args.iter().cloned());

        let completion = spawn("bwrap", &args, self.timeout, label)?;
        finish(completion, self.timeout, label, "")
    }

    fn run_container(
        &self,
        image: &str,
        command: &str,
        command_args: &[String],
        cwd: &str,
        env: &std::collections::BTreeMap<String, String>,
        label: &str,
    ) -> Result<()> {
        let cidfile = self.timeout.map(|_| cidfile_path());
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--network=none".to_string(),
            "--read-only".to_string(),
            "--cap-drop=all".to_string(),
            "--security-opt=no-new-privileges".to_string(),
            "--pids-limit=1024".to_string(),
            "--userns=keep-id".to_string(),
            "--tmpfs".to_string(),
            "/tmp:rw,size=2g".to_string(),
            "-v".to_string(),
            format!("{}:/tmp/build:Z", self.workspace.display()),
            "-v".to_string(),
            format!("{}:/tmp/stage:Z", self.stage.display()),
            "-w".to_string(),
            cwd.to_string(),
        ];
        if let Some(cidfile) = &cidfile {
            args.push("--cidfile".to_string());
            args.push(cidfile.display().to_string());
        }
        for (key, value) in base_env(self) {
            args.push("-e".to_string());
            args.push(format!("{key}={value}"));
        }
        for (key, value) in env {
            args.push("-e".to_string());
            args.push(format!("{key}={}", expand(value, self)));
        }
        args.push("--entrypoint".to_string());
        args.push(command.to_string());
        args.push(image.to_string());
        args.extend(command_args.iter().cloned());

        let completion = spawn("podman", &args, self.timeout, label)?;
        if timed_out(&completion, self.timeout) {
            if let Some(cidfile) = &cidfile {
                kill_container(cidfile);
            }
        }
        let result = finish(completion, self.timeout, label, "container ");
        if let Some(cidfile) = &cidfile {
            let _ = std::fs::remove_file(cidfile);
        }
        result
    }
}

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

/// `timeout` reports 124 when the child dies from the deadline signal, but 137
/// or a signal when the kill-after grace fires. The wall clock is the only
/// reliable signal, and it also covers a child that ignores TERM.
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

fn cidfile_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rpmx-cid-{}-{}",
        std::process::id(),
        CID_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn kill_container(cidfile: &Path) {
    let Ok(contents) = std::fs::read_to_string(cidfile) else {
        return;
    };
    let id = contents.trim();
    if id.is_empty() {
        return;
    }
    let _ = Command::new("podman")
        .args(["kill", "--signal", "KILL", id])
        .status();
    let _ = Command::new("podman").args(["rm", "-f", id]).status();
}

fn base_env(sandbox: &Sandbox<'_>) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("PATH", SANDBOX_PATH.to_string()),
        ("HOME", "/tmp/build".to_string()),
        ("TMPDIR", "/tmp".to_string()),
        ("LANG", "C.UTF-8".to_string()),
        ("LC_ALL", "C.UTF-8".to_string()),
        ("TZ", "UTC".to_string()),
        ("SOURCE_DATE_EPOCH", sandbox.source_date_epoch.to_string()),
        ("SRC", "/tmp/build".to_string()),
        ("BUILD", "/tmp/build".to_string()),
        ("STAGE", "/tmp/stage".to_string()),
        ("JOBS", sandbox.jobs.to_string()),
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
        .replace("${source.dir}", "/tmp/build")
        .replace("${build.dir}", "/tmp/build")
        .replace("${stage}", "/tmp/stage")
        .replace("${prefix}", "/usr")
        .replace("${libdir}", "/usr/lib64")
        .replace("${jobs}", &sandbox.jobs.to_string())
}
