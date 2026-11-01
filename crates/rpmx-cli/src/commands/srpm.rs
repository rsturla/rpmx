use std::path::Path;

use crate::commands::emit;
use rpmx_core::error::codes;
use rpmx_core::root::load_lock;
use rpmx_core::source_package::{prepare_source_packages, SourcePackageOptions};
use rpmx_core::Error;

use crate::commands::{
    load_plan, reconcile_policy, resolve_policy, step_root, PlanPolicy, PolicyInput, SrpmArgs,
};

pub fn run(args: SrpmArgs, pipeline_dir: Option<&Path>) -> Result<Vec<std::path::PathBuf>, Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    let signing = crate::commands::load_signing_key(
        args.signing.signing_key.as_deref(),
        args.signing.signing_passphrase_env.as_deref(),
    )?;
    let root_lock = match args.root_lock.as_deref() {
        Some(path) => {
            let lock = load_lock(path)?;
            lock.require_target(&args.arch)?;
            lock.require_digest().map_err(|error| {
                Error::new(error.code, format!("{}: {}", path.display(), error.message))
            })?;
            Some(lock)
        }
        None => None,
    };
    let policy = match root_lock.as_ref().and_then(|lock| lock.dist_tag.as_deref()) {
        Some(tag) => reconcile_policy(
            policy,
            PlanPolicy {
                dist_tag: Some(tag),
                build_release: None,
            },
        )?,
        None => policy,
    };
    let (recipe, mut plan) =
        load_plan(args.file.as_path(), &args.arch, pipeline_dir, false, policy)?;
    if let Some(id) = args.cell.as_deref() {
        plan.cells.retain(|planned| planned.id == id);
        if plan.cells.is_empty() {
            return Err(Error::new(codes::BUILD_002, format!("no such cell '{id}'")));
        }
    }
    let source_date_epoch = crate::commands::source_date_epoch(&recipe, args.source_date_epoch);
    let signature_timestamp = crate::commands::signature_timestamp(
        args.signing.signing_timestamp,
        source_date_epoch,
        signing.as_ref(),
    )?;
    let recipe_bytes = std::fs::read(&args.file).map_err(|error| {
        Error::new(
            codes::SRPM_001,
            format!("cannot read {}: {error}", args.file.display()),
        )
    })?;
    let root = step_root(pipeline_dir, &args.file);
    let debuginfo = root_lock.as_ref().is_some_and(|lock| lock.debuginfo);
    let sbom = root_lock.as_ref().map(|lock| lock.sbom).unwrap_or(true);
    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &recipe,
        plan: &plan,
        recipe_bytes: &recipe_bytes,
        step_root: root.as_deref(),
        sources: &args.sources,
        source_date_epoch,
        root_lock: root_lock.as_ref(),
        debuginfo,
        sbom,
    })?;

    std::fs::create_dir_all(&args.output).map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot create {}: {error}", args.output.display()),
        )
    })?;

    let mut written = Vec::new();
    for package in packages {
        written.push(emit::source(
            &package,
            &args.output,
            signing.as_ref(),
            signature_timestamp,
        )?);
    }

    if !args.quiet {
        crate::commands::report::print(
            args.json,
            &[crate::commands::report::ArtifactGroup {
                key: "srpms",
                paths: &written,
            }],
            signing.as_ref().map(|key| key.fingerprint()),
            true,
        )?;
    }
    Ok(written)
}
