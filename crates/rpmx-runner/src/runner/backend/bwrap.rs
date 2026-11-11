use std::path::{Path, PathBuf};

use super::process::{finish, spawn};
use crate::runner::{Runner, RunnerConfig, RunnerRequest, Session};
use rpmx_base::arch::Arch;
use rpmx_base::error::Result;

pub struct BwrapBackend<'a> {
    pub root: Option<&'a Path>,
}

impl Runner for BwrapBackend<'_> {
    fn name(&self) -> &'static str {
        "bubblewrap"
    }

    fn supports(&self, host: Arch, exec: Arch) -> bool {
        exec == host || (self.root.is_some() && crate::runner::emulation_available(exec))
    }

    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>> {
        Ok(Box::new(BwrapSession {
            root: self
                .root
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/")),
            workspace: config.workspace.to_path_buf(),
            stage: config.stage.to_path_buf(),
            sysroot: config.sysroot.map(Path::to_path_buf),
            writable_root: config.writable_root,
        }))
    }
}

struct BwrapSession {
    writable_root: bool,
    root: PathBuf,
    sysroot: Option<PathBuf>,
    workspace: PathBuf,
    stage: PathBuf,
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
            if self.writable_root {
                "--bind"
            } else {
                "--ro-bind"
            }
            .to_string(),
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
            "--chmod".to_string(),
            "1777".to_string(),
            "/tmp".to_string(),
            "--bind".to_string(),
            self.workspace.display().to_string(),
            rpmx_base::guest::BUILD.to_string(),
            "--bind".to_string(),
            self.stage.display().to_string(),
            rpmx_base::guest::STAGE.to_string(),
        ];
        if self.writable_root {
            // A transaction owns a private root. Namespace root must be able
            // to update protected account databases, including mode-000 files.
            args.extend(
                ["--uid", "0", "--gid", "0", "--cap-add", "CAP_DAC_OVERRIDE"].map(String::from),
            );
        }
        if let Some(sysroot) = &self.sysroot {
            args.push("--ro-bind".to_string());
            args.push(sysroot.display().to_string());
            args.push(rpmx_base::guest::SYSROOT.to_string());
        }
        args.extend(["--chdir".to_string(), request.cwd.to_string()]);
        for (key, value) in request.env {
            args.push("--setenv".to_string());
            args.push(key.clone());
            args.push(value.clone());
        }
        args.push("--".to_string());
        args.push(request.command.to_string());
        args.extend(request.args.iter().cloned());

        let completion = spawn("bwrap", &args, request.timeout, request.label, request.logs)?;
        finish(completion, request.timeout, request.label, "")
    }

    fn sync_out(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::num::NonZeroU64;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    // Copy only the executables and their loader dependencies into a disposable
    // root. In particular, a writable session must never bind the host root.
    fn copy_executable(root: &Path, executable: &str) {
        let dependencies = Command::new("ldd").arg(executable).output().unwrap();
        assert!(dependencies.status.success());
        let output = String::from_utf8(dependencies.stdout).unwrap();
        for name in std::iter::once(executable).chain(
            output
                .split_whitespace()
                .filter(|word| word.starts_with('/')),
        ) {
            let destination = root.join(name.trim_start_matches('/'));
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(name, destination).unwrap();
        }
    }

    #[test]
    fn private_transactions_can_update_protected_files_and_tmp_is_shared() {
        if !Command::new("bwrap")
            .args(["--unshare-all", "--ro-bind", "/", "/", "--", "true"])
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("bubblewrap is unavailable; skip the sandbox execution test");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        copy_executable(&root, "/bin/sh");
        copy_executable(&root, "/usr/bin/stat");
        for path in ["dev", "proc", "sys", "tmp"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        std::fs::create_dir_all(root.join("etc")).unwrap();
        let protected = root.join("etc/protected");
        std::fs::write(&protected, "original\n").unwrap();
        std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o0)).unwrap();
        let workspace = directory.path().join("build");
        let stage = directory.path().join("stage");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&stage).unwrap();
        let mut session = BwrapSession {
            writable_root: false,
            root,
            sysroot: None,
            workspace,
            stage,
        };
        let environment = BTreeMap::new();
        let run = |session: &mut BwrapSession, script: &str| {
            session.run(&RunnerRequest {
                timeout: NonZeroU64::new(10).unwrap(),
                command: "/bin/sh",
                args: &["-ec".into(), script.into()],
                cwd: rpmx_base::guest::BUILD,
                env: &environment,
                label: "sandbox permissions",
                logs: None,
            })
        };
        run(&mut session, "test \"$(/usr/bin/stat -c %a /tmp)\" = 1777; if (read -r value </etc/protected); then exit 1; fi").unwrap();
        session.writable_root = true;
        run(&mut session, "read -r value </etc/protected; test \"$value\" = original; printf 'changed\\n' >/etc/protected").unwrap();
        std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read_to_string(protected).unwrap(), "changed\n");
    }
}
