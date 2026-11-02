use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::source::{assemble, SourceOptions};
use rpmx_core::Error;

use crate::commands::fetch::default_cache;
use crate::commands::{load_plan, resolve_policy, PlanPolicy, PolicyInput, SourceArgs};

/// Everything source assembly needs, independent of clap.
pub struct SourceRequest<'a> {
    pub file: &'a Path,
    pub arch: &'a str,
    /// Where steps execute: the host for a cross build, the target otherwise.
    pub exec: rpmx_core::arch::Arch,
    /// Target sysroot, mounted read-only, for a cross build.
    pub sysroot: Option<&'a Path>,
    /// Toolchain commands for a cross build.
    pub cross: Option<&'a rpmx_core::guest::CrossToolchain>,
    pub runner: &'a str,
    pub policy: PlanPolicy<'a>,
    pub profile: Option<&'a Path>,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub locked: bool,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub runner_asset_root: Option<&'a Path>,
    pub unsafe_host_root: bool,
    /// Re-hash an explicit root. A caller that already resolved the root
    /// passes false to avoid hashing the whole tree twice.
    pub verify_root: bool,
    pub output: &'a Path,
    pub source_date_epoch: Option<u32>,
    pub jobs: NonZeroU32,
    pub step_timeout: NonZeroU64,
    pub quiet: bool,
    pub pipeline_dir: Option<&'a Path>,
}

pub fn run(args: SourceArgs, pipeline_dir: Option<&Path>) -> Result<PathBuf, Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    execute(SourceRequest {
        file: &args.file,
        arch: &args.arch,
        exec: rpmx_core::arch::Arch::parse(&args.arch)?,
        sysroot: None,
        cross: None,
        runner: &args.runner,
        policy,
        profile: args.policy.profile.as_deref(),
        cache: args.runtime.cache.cache.as_deref(),
        proxy: args.proxy.as_deref(),
        offline: args.offline,
        locked: args.locked,
        root_lock: args.root_lock.as_deref(),
        root: args.root.as_deref(),
        runner_asset_root: args.runner_asset_root.as_deref(),
        unsafe_host_root: args.unsafe_host_root,
        verify_root: true,
        output: &args.output,
        source_date_epoch: args.source_date_epoch,
        jobs: args.runtime.jobs,
        step_timeout: args.runtime.step_timeout,
        quiet: args.quiet,
        pipeline_dir,
    })
}

pub fn execute(request: SourceRequest<'_>) -> Result<PathBuf, Error> {
    let cache = match request.cache {
        Some(path) => path.to_path_buf(),
        None => default_cache()?,
    };
    let proxy = request
        .proxy
        .map(str::to_string)
        .or_else(|| std::env::var("RPMX_PROXY").ok())
        .or_else(|| std::env::var("RPMX_ACQUISITION_PROXY").ok());
    let (bundle, plan) = load_plan(
        request.file,
        request.arch,
        request.pipeline_dir,
        false,
        request.policy,
    )?;
    let automatic =
        if request.root_lock.is_none() && request.root.is_none() && !request.unsafe_host_root {
            crate::commands::autoroot::ensure(
                &bundle.recipe,
                &plan,
                &bundle.root,
                request.profile,
                request.cache,
                request.offline,
                false,
                request.arch,
            )?
        } else {
            None
        };
    if let Some(root) = &automatic {
        crate::commands::autoroot::report_root(
            &root.document,
            &root.profile,
            if root.cached {
                "cache hit"
            } else {
                "assembled"
            },
        );
    }
    let verify_digest = request.verify_root && automatic.is_none();
    let automatic_lock = automatic.as_ref().map(|root| root.lock.as_path());
    let automatic_root = automatic.as_ref().map(|root| root.root.as_path());
    let root_lock = request.root_lock.or(automatic_lock);
    let root = request.root.or(automatic_root);
    if request.locked {
        let active_lock = root_lock.map(rpmx_core::root::load_lock).transpose()?;
        crate::commands::lock::verify(
            &bundle,
            &plan,
            request.policy,
            request.profile,
            request.arch,
            request.source_date_epoch,
            active_lock.as_ref(),
            true,
        )?;
    }
    let source_date_epoch =
        crate::commands::source_date_epoch(&bundle.recipe, request.source_date_epoch);
    let work = crate::commands::WorkDir::new("source")?;
    let workspace = work.path().join("build");
    let stage = work.path().join("stage");
    let base_dir = request.file.parent().unwrap_or_else(|| Path::new("."));

    let root_request = rpmx_core::request::resolve(rpmx_core::request::RootRequestOptions {
        lock: root_lock,
        root,
        allow_host: request.unsafe_host_root,
        arch: request.arch,
        verify_digest,
    })?;
    let selected = match &root_request {
        rpmx_core::request::RootRequest::Host => None,
        rpmx_core::request::RootRequest::Locked { lock, path, digest } => {
            Some(rpmx_core::root::RootSelection {
                lock: (**lock).clone(),
                path: path.clone(),
                digest: digest.clone(),
            })
        }
    };
    let selected_root = selected.as_ref().map(|selection| selection.path.as_path());
    let prefix = selected
        .as_ref()
        .map(|selection| selection.lock.prefix.as_str())
        .unwrap_or(rpmx_core::guest::PREFIX);
    let libdir = selected
        .as_ref()
        .map(|selection| selection.lock.libdir.as_str())
        .unwrap_or(rpmx_core::guest::LIBDIR);
    let appliance = crate::commands::autoroot::appliance_for_lock(
        root_lock,
        request.cache,
        request.offline,
        request.runner.parse::<rpmx_core::runner::RunnerKind>()?,
        request.arch,
        request.runner_asset_root,
    )?;

    let _phase = rpmx_core::output::enter_phase("source");
    rpmx_core::output::phase(
        "source",
        format!("{} assembling RPMX-SOURCE.tar.gz", request.arch),
    );
    let source_started = std::time::Instant::now();
    let snapshot = assemble(SourceOptions {
        recipe: &bundle.recipe,
        plan: &plan,
        cache: &cache,
        proxy: proxy.as_deref(),
        offline: request.offline,
        base_dir,
        workspace: &workspace,
        stage: &stage,
        prefix,
        libdir,
        output: request.output,
        source_date_epoch,
        jobs: request.jobs,
        step_timeout: request.step_timeout,
        root: selected_root,
        appliance: appliance.as_deref(),
        exec: request.exec,
        sysroot: request.sysroot,
        cross: request.cross,
        runner: request.runner,
        recipe_dir: request.file.parent().unwrap_or_else(|| Path::new(".")),
    })?;
    rpmx_core::output::phase(
        "source",
        format!(
            "{} {} · {}",
            request.arch,
            snapshot
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("RPMX-SOURCE.tar.gz"),
            rpmx_core::output::duration(source_started)
        ),
    );

    if let Some(selection) = &selected {
        let mut text = serde_json::to_string_pretty(&selection.lock)
            .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?;
        text.push('\n');
        let path = request.output.join("source-root.lock.json");
        std::fs::write(&path, text).map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }

    if !request.quiet {
        if rpmx_core::output::json() {
            let value = crate::commands::report::document(
                "source",
                &[crate::commands::report::ArtifactGroup {
                    kind: "source-snapshot",
                    paths: std::slice::from_ref(&snapshot),
                }],
                None,
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&value)
                    .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
            );
        } else {
            println!("{}", snapshot.display());
        }
        rpmx_core::output::phase(
            "done",
            format!(
                "{} in {} · {}",
                rpmx_core::output::count(1, "artifact", "artifacts"),
                rpmx_core::output::display_path(request.output),
                rpmx_core::output::duration(source_started)
            ),
        );
    }
    Ok(snapshot)
}
