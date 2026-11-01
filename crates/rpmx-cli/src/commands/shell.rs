//! Enter the same bubblewrap sandbox a build step uses.
//!
//! Pair with `--keep-workdir`: the build prints the workspace and stage, and
//! this command opens them in an identical mount layout.

use std::process::Command;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::ShellArgs;

pub fn run(args: ShellArgs) -> Result<(), Error> {
    for path in [&args.root, &args.workspace, &args.stage] {
        if !path.is_dir() {
            return Err(Error::new(
                codes::ROOT_016,
                format!("{} is not a directory", path.display()),
            ));
        }
    }
    let root = args.root.display().to_string();
    let workspace = args.workspace.display().to_string();
    let stage = args.stage.display().to_string();
    let mut bwrap = Command::new("bwrap");
    bwrap.args([
        "--unshare-all",
        "--unshare-user",
        "--disable-userns",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--ro-bind",
        &root,
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/sys",
        "--tmpfs",
        "/tmp",
        "--bind",
        &workspace,
        rpmx_core::guest::BUILD,
        "--bind",
        &stage,
        rpmx_core::guest::STAGE,
        "--chdir",
        rpmx_core::guest::BUILD,
        "--setenv",
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "--setenv",
        "HOME",
        rpmx_core::guest::BUILD,
        "--setenv",
        "STAGE",
        rpmx_core::guest::STAGE,
        "--",
    ]);
    match args.command.as_deref() {
        Some(command) => {
            bwrap.args(["sh", "-c", command]);
        }
        None => {
            bwrap.args(["sh", "-i"]);
        }
    }
    let status = bwrap.status().map_err(|error| {
        Error::new(
            codes::SANDBOX_001,
            format!("cannot start bubblewrap: {error}"),
        )
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::new(
            codes::SANDBOX_002,
            format!("shell exited with {status}"),
        ))
    }
}
