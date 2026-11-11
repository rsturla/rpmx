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

struct ShellRoot {
    root: PathBuf,
    sysroot: Option<PathBuf>,
    cross: Option<rpmx_core::guest::CrossToolchain>,
    prefix: String,
    libdir: String,
}

pub fn run(args: ShellArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    let arch = match (&args.arch, &args.root) {
        (Some(arch), _) => arch.clone(),
        // An explicit root carries no cross context, so the recipe is not
        // needed to choose an architecture.
        (None, Some(_)) => rpmx_core::arch::native_arch().to_string(),
        (None, None) => {
            let declared = rpmx_core::load_recipe_bundle(&args.file)?;
            crate::commands::autoroot::declared_arch(
                &declared.recipe,
                &declared.root,
                args.policy.profile.as_deref(),
            )?
            .unwrap_or_else(|| rpmx_core::arch::native_arch().to_string())
        }
    };
    let shell_root = match args.root.clone() {
        Some(root) => ShellRoot {
            root,
            sysroot: None,
            cross: None,
            prefix: rpmx_core::guest::PREFIX.to_string(),
            libdir: rpmx_core::guest::LIBDIR.to_string(),
        },
        None => resolve_root(&args, &arch, pipeline_dir)?,
    };
    let root = shell_root.root;
    let sysroot = shell_root.sysroot;
    let cross = shell_root.cross;
    let prefix = shell_root.prefix;
    let libdir = shell_root.libdir;
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
    ]);
    let sysroot_path = sysroot.as_ref().map(|path| path.display().to_string());
    if let Some(path) = &sysroot_path {
        bwrap.args(["--ro-bind", path, rpmx_core::guest::SYSROOT]);
    }
    bwrap.args(["--chdir", rpmx_core::guest::BUILD]);
    // The same environment a build step gets, not a hand-copied subset.
    let config = rpmx_core::sandbox::SandboxConfig {
        writable_root: false,
        runner: rpmx_core::runner::RunnerKind::Auto,
        host: rpmx_core::Arch::current()?,
        exec: rpmx_core::Arch::parse(&arch)?,
        target: rpmx_core::Arch::parse(&arch)?,
        sysroot: sysroot.as_deref(),
        cross: cross.as_ref(),
        workspace: std::path::Path::new(&workspace),
        stage: std::path::Path::new(&stage),
        root: Some(std::path::Path::new(&root)),
        assets: rpmx_core::runner::RunnerAssets::default(),
        image: None,
        prefix: &prefix,
        libdir: &libdir,
        source_date_epoch: 0,
        jobs: crate::commands::default_jobs(),
        timeout: std::num::NonZeroU64::new(3600).expect("timeout"),
    };
    for (key, value) in rpmx_core::sandbox::environment(&config) {
        bwrap.arg("--setenv").arg(key).arg(value);
    }
    bwrap.arg("--");
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

fn resolve_root(
    args: &ShellArgs,
    arch: &str,
    pipeline_dir: Option<&Path>,
) -> Result<ShellRoot, Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let (bundle, plan) = load_plan(
        &args.file,
        arch,
        pipeline_dir,
        false,
        true,
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
        arch,
    )?;
    automatic
        .map(|root| ShellRoot {
            sysroot: root.sysroot.as_ref().map(|sysroot| sysroot.root.clone()),
            cross: root.cross.clone(),
            prefix: root.document.prefix.clone(),
            libdir: root.document.libdir.clone(),
            root: root.root,
        })
        .ok_or_else(|| {
            Error::new(
                codes::ROOT_016,
                "no build root: set build.root in the recipe, pass --profile, or pass --root",
            )
        })
}
