use crate::commands::emit;
use rpmx_core::error::codes;
use rpmx_core::root::load_lock;
use rpmx_core::source_package::{prepare_source_packages, SourcePackageOptions};
use rpmx_core::Error;

use crate::commands::{load_plan, reconcile_policy, step_root, PlanPolicy, SrpmArgs};

pub fn run(args: SrpmArgs<'_>) -> Result<Vec<std::path::PathBuf>, Error> {
    let SrpmArgs {
        file,
        sources,
        output,
        arch,
        source_date_epoch,
        cell,
        pipeline_dir,
        policy,
        root_lock,
        signing_key,
        signing_passphrase_env,
        signing_timestamp,
        quiet,
        json,
    } = args;
    let signing = crate::commands::load_signing_key(signing_key, signing_passphrase_env)?;
    let root_lock = match root_lock {
        Some(path) => {
            let lock = load_lock(path)?;
            lock.require_target(arch)?;
            lock.require_digest().map_err(|error| {
                Error::new(error.code, format!("{}: {}", path.display(), error.message))
            })?;
            Some(lock)
        }
        None => None,
    };
    // A root lock is a target-distro statement. It supplies the dist tag when
    // the profile and command line did not, and conflicts are errors rather
    // than silently producing contradictory provenance.
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
    let (recipe, mut plan) = load_plan(file, arch, pipeline_dir, false, policy)?;
    if let Some(id) = cell {
        plan.cells.retain(|planned| planned.id == id);
        if plan.cells.is_empty() {
            return Err(Error::new(codes::BUILD_002, format!("no such cell '{id}'")));
        }
    }
    let source_date_epoch = crate::commands::source_date_epoch(&recipe, source_date_epoch);
    let signature_timestamp = crate::commands::signature_timestamp(
        signing_timestamp,
        source_date_epoch,
        signing.as_ref(),
    )?;
    let recipe_bytes = std::fs::read(file).map_err(|error| {
        Error::new(
            codes::SRPM_001,
            format!("cannot read {}: {error}", file.display()),
        )
    })?;
    let root = step_root(pipeline_dir, file);
    let debuginfo = root_lock.as_ref().is_some_and(|lock| lock.debuginfo);
    let sbom = root_lock.as_ref().map(|lock| lock.sbom).unwrap_or(true);
    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &recipe,
        plan: &plan,
        recipe_bytes: &recipe_bytes,
        step_root: root.as_deref(),
        sources,
        source_date_epoch,
        root_lock: root_lock.as_ref(),
        debuginfo,
        sbom,
    })?;

    std::fs::create_dir_all(output).map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot create {}: {error}", output.display()),
        )
    })?;

    let mut written = Vec::new();
    for package in packages {
        written.push(emit::source(
            &package,
            output,
            signing.as_ref(),
            signature_timestamp,
        )?);
    }

    if !quiet {
        crate::commands::report::print(
            json,
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
