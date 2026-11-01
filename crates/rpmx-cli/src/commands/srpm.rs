use std::path::{Path, PathBuf};

use crate::commands::emit;
use rpmx_core::error::codes;
use rpmx_core::root::load_lock;
use rpmx_core::source_package::{prepare_source_packages, SourcePackageOptions};
use rpmx_core::Error;

use crate::commands::{
    load_plan, reconcile_policy, resolve_policy, step_root, OptionalSigningArgs, PlanPolicy,
    PolicyInput, SrpmArgs,
};

/// Everything SRPM assembly needs, independent of clap.
pub struct SrpmRequest<'a> {
    pub file: &'a Path,
    pub policy: PlanPolicy<'a>,
    pub sources: &'a Path,
    pub output: &'a Path,
    pub arch: &'a str,
    pub source_date_epoch: Option<u32>,
    pub cell: Option<&'a str>,
    pub root_lock: Option<&'a Path>,
    pub signing: &'a OptionalSigningArgs,
    pub quiet: bool,
    pub pipeline_dir: Option<&'a Path>,
}

pub fn run(args: SrpmArgs, pipeline_dir: Option<&Path>) -> Result<Vec<PathBuf>, Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    let work;
    let sources = match args.sources.as_deref() {
        Some(path) => path.to_path_buf(),
        None => {
            work = crate::commands::WorkDir::new("srpm-source")?;
            let output = work.path().join("source");
            std::fs::create_dir_all(&output).map_err(|error| {
                Error::new(
                    codes::SRPM_002,
                    format!("cannot create {}: {error}", output.display()),
                )
            })?;
            crate::commands::source::execute(crate::commands::source::SourceRequest {
                file: &args.file,
                arch: &args.arch,
                runner: &args.runner,
                policy,
                profile: args.policy.profile.as_deref(),
                cache: args.runtime.cache.cache.as_deref(),
                proxy: args.proxy.as_deref(),
                offline: args.offline,
                locked: false,
                root_lock: args.root_lock.as_deref(),
                root: args.root.as_deref(),
                unsafe_host_root: args.unsafe_host_root,
                verify_root: true,
                output: &output,
                source_date_epoch: args.source_date_epoch,
                jobs: args.runtime.jobs,
                step_timeout: args.runtime.step_timeout,
                quiet: true,
                pipeline_dir,
            })?;
            output
        }
    };
    execute(SrpmRequest {
        file: &args.file,
        policy,
        sources: &sources,
        output: &args.output,
        arch: &args.arch,
        source_date_epoch: args.source_date_epoch,
        cell: args.cell.as_deref(),
        root_lock: args.root_lock.as_deref(),
        signing: &args.signing,
        quiet: args.quiet,
        pipeline_dir,
    })
}

pub fn execute(request: SrpmRequest<'_>) -> Result<Vec<PathBuf>, Error> {
    let signing = crate::commands::load_signing_key(
        request.signing.signing_key.as_deref(),
        request.signing.signing_passphrase_env.as_deref(),
    )?;
    let root_lock = match request.root_lock {
        Some(path) => {
            let lock = load_lock(path)?;
            lock.require_target(request.arch)?;
            lock.require_digest().map_err(|error| {
                Error::new(error.code, format!("{}: {}", path.display(), error.message))
            })?;
            Some(lock)
        }
        None => None,
    };
    let policy = match root_lock.as_ref().and_then(|lock| lock.dist_tag.as_deref()) {
        Some(tag) => reconcile_policy(
            request.policy,
            PlanPolicy {
                dist_tag: Some(tag),
                build_release: None,
            },
        )?,
        None => request.policy,
    };
    let (bundle, mut plan) = load_plan(
        request.file,
        request.arch,
        request.pipeline_dir,
        false,
        policy,
    )?;
    if let Some(id) = request.cell {
        plan.cells.retain(|planned| planned.id == id);
        if plan.cells.is_empty() {
            return Err(Error::new(codes::BUILD_002, format!("no such cell '{id}'")));
        }
    }
    let source_date_epoch =
        crate::commands::source_date_epoch(&bundle.recipe, request.source_date_epoch);
    let signature_timestamp = crate::commands::signature_timestamp(
        request.signing.signing_timestamp,
        source_date_epoch,
        signing.as_ref(),
    )?;
    let recipe_bytes = bundle.root_file().bytes.clone();
    let recipe_includes: Vec<rpmx_core::source_package::RecipeInclude> = bundle
        .included_files()
        .map(|file| rpmx_core::source_package::RecipeInclude {
            name: file.name.clone(),
            bytes: file.bytes.clone(),
        })
        .collect();
    let root = step_root(request.pipeline_dir, &bundle.root);
    let debuginfo = root_lock.as_ref().is_some_and(|lock| lock.debuginfo);
    let sbom = root_lock.as_ref().map(|lock| lock.sbom).unwrap_or(true);
    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &bundle.recipe,
        plan: &plan,
        recipe_bytes: &recipe_bytes,
        recipe_digest: Some(bundle.digest()),
        recipe_includes: &recipe_includes,
        step_root: root.as_deref(),
        sources: request.sources,
        source_date_epoch,
        root_lock: root_lock.as_ref(),
        debuginfo,
        sbom,
    })?;

    std::fs::create_dir_all(request.output).map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot create {}: {error}", request.output.display()),
        )
    })?;

    let mut written = Vec::new();
    for package in packages {
        written.push(emit::source(
            &package,
            request.output,
            signing.as_ref(),
            signature_timestamp,
        )?);
    }

    if !request.quiet {
        crate::commands::report::print(
            rpmx_core::output::json(),
            "srpm",
            &[crate::commands::report::ArtifactGroup {
                kind: "srpm",
                paths: &written,
            }],
            signing.as_ref().map(|key| key.fingerprint()),
        )?;
    }
    Ok(written)
}
