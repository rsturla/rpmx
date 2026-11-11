use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::process::{finish_for, spawn_inheriting_fds, spawn_with_fds_for, timed_out_for};
use crate::runner::{Runner, RunnerConfig, RunnerRequest, Session};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};

static CID_COUNTER: AtomicU64 = AtomicU64::new(0);
// Storage cleanup can wait behind concurrent image/root operations. Give it
// a bounded grace period; execution deadlines still determine case success.
const CLEANUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

pub enum PodmanSource<'a> {
    Image(&'a str),
    Root(&'a std::path::Path),
}

pub struct PodmanBackend<'a> {
    pub source: PodmanSource<'a>,
    /// Architecture the container executes. It is the execution architecture,
    /// not the package target: a cross build runs a host container.
    pub arch: Arch,
}

impl Runner for PodmanBackend<'_> {
    fn name(&self) -> &'static str {
        "podman"
    }

    fn supports(&self, host: Arch, exec: Arch) -> bool {
        host == exec
    }

    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>> {
        let name = format!(
            "rpmx-run-{}-{}",
            std::process::id(),
            CID_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            name.clone(),
            "--arch".to_string(),
            self.arch.to_string(),
            if config.policy.network {
                "--network=host"
            } else {
                "--network=none"
            }
            .to_string(),
            "--cap-drop=all".to_string(),
            "--security-opt=no-new-privileges".to_string(),
            "--pids-limit=1024".to_string(),
            if config.writable_root {
                "--userns=keep-id:uid=0,gid=0"
            } else {
                "--userns=keep-id:uid=65534,gid=65534"
            }
            .to_string(),
            if config.writable_root {
                "--user=0:0"
            } else {
                "--user=65534:65534"
            }
            .to_string(),
            "--tmpfs".to_string(),
            "/tmp:rw,size=2g".to_string(),
            "-v".to_string(),
            format!("{}:{}", config.workspace.display(), rpmx_base::guest::BUILD),
            "-v".to_string(),
            format!(
                "{}:{}:{}",
                config.stage.display(),
                rpmx_base::guest::STAGE,
                if config.policy.readonly_stage {
                    "ro"
                } else {
                    "rw"
                }
            ),
        ];
        args.extend(["--security-opt=label=disable".into()]);
        if config.writable_root {
            // Account databases, payload ownership and credential-changing
            // scriptlets need this bounded transaction set. Never grant
            // SYS_ADMIN or restore the engine's default capability set.
            for capability in crate::runner::TRANSACTION_CAPABILITIES {
                args.push(format!("--cap-add={capability}"));
            }
        } else {
            args.push("--read-only".into());
        }
        if let Some(sysroot) = config.sysroot {
            args.extend([
                "-v".into(),
                format!("{}:{}:ro", sysroot.display(), rpmx_base::guest::SYSROOT),
            ]);
        }
        for mount in config.policy.readonly_mounts {
            if !mount.target.starts_with("/tmp/rpmx-inputs/")
                || mount.target.split('/').any(|part| part == "..")
            {
                return Err(Error::new(
                    codes::SANDBOX_003,
                    "test input mounts must stay below /tmp/rpmx-inputs",
                ));
            }
            args.extend([
                "-v".into(),
                format!("{}:{}:ro", mount.source.display(), mount.target),
            ]);
        }
        args.extend(["--entrypoint".into(), "/bin/sh".into()]);
        match self.source {
            PodmanSource::Image(image) => args.push(image.into()),
            PodmanSource::Root(root) => {
                args.extend(["--rootfs".into(), format!("{}:O", root.display())])
            }
        }
        args.extend(["-c".into(), "trap : TERM INT; sleep infinity & wait".into()]);
        let startup_budget = config
            .policy
            .deadline
            .map(|deadline| deadline.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or_else(|| std::time::Duration::from_secs(config.timeout.get()));
        let output = control_output(&args, startup_budget).map_err(|error| {
            Error::new(codes::SANDBOX_001, format!("cannot start podman: {error}"))
        })?;
        if !output.status.success() {
            // A timed-out client can leave a started container behind. Its
            // unique name is retained until bounded cleanup has been attempted.
            let _ = control_output(&["rm".into(), "-f".into(), name.clone()], CLEANUP_BUDGET);
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
            stopped: false,
            deadline: config.policy.deadline,
        }))
    }
}

struct PodmanSession {
    name: String,
    stopped: bool,
    deadline: Option<std::time::Instant>,
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

        let timeout = self
            .deadline
            .map(|deadline| {
                deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .min(std::time::Duration::from_secs(request.timeout.get()))
            })
            .unwrap_or_else(|| std::time::Duration::from_secs(request.timeout.get()));
        let completion =
            spawn_with_fds_for("podman", &args, timeout, request.label, request.logs, &[])?;
        if timed_out_for(&completion, timeout) {
            // finish() retries cleanup if removal itself failed; successful
            // removal is recorded so it is never mistaken for a second failure.
            let _ = self.stop();
        }
        finish_for(completion, timeout, request.label, "container ")
    }

    fn sync_out(&mut self) -> Result<()> {
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.stop()
    }

    fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        let output = control_output(
            &["rm".into(), "-f".into(), self.name.clone()],
            CLEANUP_BUDGET,
        )
        .map_err(|error| {
            Error::new(
                codes::SANDBOX_001,
                format!("cannot stop podman session: {error}"),
            )
        })?;
        if !output.status.success() {
            return Err(Error::new(
                codes::SANDBOX_002,
                format!(
                    "cannot stop podman session ({}; cleanup budget {}s): {}",
                    output.status,
                    CLEANUP_BUDGET.as_secs(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        self.stopped = true;
        Ok(())
    }
}
impl Drop for PodmanSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// Infrastructure commands are bounded too: a stalled engine must not consume
// the case's deadline indefinitely or inherit unrelated caller descriptors.
fn control_output(
    args: &[String],
    budget: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    let mut command = Command::new("timeout");
    command
        .args([
            "--signal=TERM",
            "--kill-after=1s",
            &format!("{:.9}s", budget.as_secs_f64().max(0.000_000_001)),
            "podman",
        ])
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    spawn_inheriting_fds(command, &[])?.wait_with_output()
}
