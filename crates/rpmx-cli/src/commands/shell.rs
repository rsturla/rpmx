//! Enter the same bubblewrap sandbox a build step uses.
//!
//! The root comes from the recipe's `build.root` unless `--root` is given.
//! The workspace defaults to the current directory and the stage to a
//! temporary directory, so the command is useful without a prior build.

use std::path::{Path, PathBuf};
use std::process::Command;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{autoroot, load_plan, resolve_policy, PolicyInput, ShellArgs, WorkDir};

pub fn run(args: ShellArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    let root = match args.root.clone() {
        Some(root) => root,
        None => resolve_root(&args, pipeline_dir)?,
    };
    let workspace = match args.workspace.clone() {
        Some(workspace) => workspace,
        None => std::env::current_dir().map_err(|error| {
            Error::new(
                codes::ROOT_016,
                format!("cannot use the current directory as the workspace: {error}"),
            )
        })?,
    };
    let work = if args.stage.is_none() {
        Some(WorkDir::new("shell")?)
    } else {
        None
    };
    let stage = match args.stage.clone() {
        Some(stage) => stage,
        None => work
            .as_ref()
            .expect("a work directory exists when --stage is absent")
            .path()
            .join("stage"),
    };
    std::fs::create_dir_all(&stage).map_err(|error| {
        Error::new(
            codes::ROOT_016,
            format!("cannot create {}: {error}", stage.display()),
        )
    })?;

    for path in [&root, &workspace, &stage] {
        if !path.is_dir() {
            return Err(Error::new(
                codes::ROOT_016,
                format!("{} is not a directory", path.display()),
            ));
        }
    }
    let root = root.display().to_string();
    let workspace = workspace.display().to_string();
    let stage = stage.display().to_string();
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

fn resolve_root(args: &ShellArgs, pipeline_dir: Option<&Path>) -> Result<PathBuf, Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let (bundle, plan) = load_plan(
        &args.file,
        &args.arch,
        pipeline_dir,
        false,
        resolved.plan_policy(),
    )?;
    let automatic = autoroot::ensure(
        &bundle.recipe,
        &plan,
        &bundle.root,
        args.policy.profile.as_deref(),
        args.cache.cache.as_deref(),
        args.offline,
        false,
        &args.arch,
        rpmx_core::runner::RunnerKind::Bwrap,
        None,
    )?;
    automatic.map(|root| root.root).ok_or_else(|| {
        Error::new(
            codes::ROOT_016,
            "no build root: set build.root in the recipe, pass --profile, or pass --root",
        )
    })
}
