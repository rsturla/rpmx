use std::num::NonZeroU64;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::process::{finish, spawn, timed_out};
use crate::runner::{Runner, RunnerConfig, RunnerRequest, Session};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};

static CID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct PodmanBackend<'a> {
    pub image: &'a str,
}

impl Runner for PodmanBackend<'_> {
    fn name(&self) -> &'static str {
        "podman"
    }

    fn supports(&self, host: Arch, target: Arch) -> bool {
        host == target
    }

    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>> {
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
    timeout: NonZeroU64,
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
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
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
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        Ok(())
    }
}
