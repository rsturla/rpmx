//! Enter the same bubblewrap sandbox a build step uses.
//!
//! The root comes from the recipe's `build.root` unless `--root` is given.
//! The workspace defaults to the current directory and the stage to a
//! temporary directory, so the command is useful without a prior build.

use std::path::{Path, PathBuf};

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
    // A cross build executes host tools against a separate target sysroot.
    // A native/emulated build executes the target binaries directly.
    let host = rpmx_core::Arch::current()?;
    let target = rpmx_core::Arch::parse(&arch)?;
    let exec = if sysroot.is_some() { host } else { target };
    // The same environment a build step gets, not a hand-copied subset.
    let config = rpmx_core::sandbox::SandboxConfig {
        writable_root: false,
        runner: rpmx_core::runner::RunnerKind::Auto,
        host,
        exec,
        target,
        sysroot: sysroot.as_deref(),
        cross: cross.as_ref(),
        workspace: &workspace,
        stage: &stage,
        root: Some(&root),
        assets: rpmx_core::runner::RunnerAssets::default(),
        image: None,
        prefix: &prefix,
        libdir: &libdir,
        source_date_epoch: 0,
        jobs: crate::commands::default_jobs(),
        timeout: std::num::NonZeroU64::new(3600).expect("timeout"),
    };
    let shell_args = match args.command {
        Some(command) => vec!["-c".to_string(), command],
        None => vec!["-i".to_string()],
    };
    rpmx_core::sandbox::run_native_terminal(&config, "sh", &shell_args)
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
