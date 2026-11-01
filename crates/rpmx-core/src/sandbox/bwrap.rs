use std::path::{Path, PathBuf};

use crate::arch::Arch;
use crate::error::Result;
use crate::sandbox::process::{finish, spawn};
use crate::sandbox::{Runner, RunnerRequest, Session, SessionConfig};

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
