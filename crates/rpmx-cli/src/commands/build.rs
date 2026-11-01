use std::path::{Path, PathBuf};

use crate::commands::emit;
use rpmx_core::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{
    load_plan, pipeline::BuildPipeline, resolve_policy, BuildArgs, PlanPolicy, PolicyInput,
};

pub fn run(args: BuildArgs, pipeline_dir: Option<&Path>) -> Result<Vec<PathBuf>, Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    if let Some(stage) = args.stage.as_deref() {
        return package_stage(&args, stage, pipeline_dir, policy);
    }
    build_from_recipe(args, pipeline_dir, policy)
}

/// Package an already-staged tree. This is the original `build --stage` mode.
fn package_stage(
    args: &BuildArgs,
    stage: &Path,
    pipeline_dir: Option<&Path>,
    policy: PlanPolicy<'_>,
) -> Result<Vec<PathBuf>, Error> {
    let signing = crate::commands::load_signing_key(
        args.signing.signing_key.as_deref(),
        args.signing.signing_passphrase_env.as_deref(),
    )?;
    let (recipe, plan) = load_plan(args.file.as_path(), &args.arch, pipeline_dir, false, policy)?;
    let source_date_epoch = crate::commands::source_date_epoch(&recipe, args.source_date_epoch);
    let signature_timestamp = crate::commands::signature_timestamp(
        args.signing.signing_timestamp,
        source_date_epoch,
        signing.as_ref(),
    )?;
    let cells: Vec<_> = match args.cell.as_deref() {
        Some(id) => plan.cells.iter().filter(|cell| cell.id == id).collect(),
        None => plan.cells.iter().collect(),
    };
    if cells.is_empty() {
        return Err(Error::new(
            codes::BUILD_002,
            format!(
                "no such cell '{}'",
                args.cell.as_deref().unwrap_or_default()
            ),
        ));
    }
    if cells.len() > 1 {
        return Err(Error::new(
            codes::BUILD_003,
            "this plan has several cells; pass --cell for each per-cell stage",
        ));
    }

    std::fs::create_dir_all(&args.output).map_err(|error| {
        Error::new(
            codes::BUILD_001,
            format!("cannot create {}: {error}", args.output.display()),
        )
    })?;

    let mut written = Vec::new();
    for cell in &cells {
        let packages = prepare_binary_packages(BinaryPackageOptions {
            recipe: &recipe,
            cell,
            stage,
            arch: &args.arch,
            source_date_epoch,
            debuginfo: false,
        })?;
        for package in packages {
            written.push(emit::binary(
                &package,
                &args.output,
                signing.as_ref(),
                signature_timestamp,
            )?);
        }
    }

    report(args.json, &written, &[])
}

/// Run the full SRPM-first pipeline: source snapshot, SRPM, binary RPMs.
fn build_from_recipe(
    args: BuildArgs,
    pipeline_dir: Option<&Path>,
    policy: PlanPolicy<'_>,
) -> Result<Vec<PathBuf>, Error> {
    let BuildArgs {
        file,
        output,
        arch,
        runner,
        source_date_epoch,
        cell,
        cache,
        proxy,
        offline,
        root_lock,
        root,
        unsafe_host_root,
        jobs,
        step_timeout,
        run_tests,
        sbom,
        test_root_lock,
        signing,
        json,
        ..
    } = args;

    let _root_request = rpmx_core::request::resolve(rpmx_core::request::RootRequestOptions {
        lock: root_lock.as_deref(),
        root: root.as_deref(),
        allow_host: unsafe_host_root,
        arch: &arch,
    })?;

    let (_, plan) = load_plan(&file, &arch, pipeline_dir, false, policy)?;
    let selected: Vec<_> = match cell.as_deref() {
        Some(id) => plan.cells.iter().filter(|cell| cell.id == id).collect(),
        None => plan.cells.iter().collect(),
    };
    if selected.is_empty() {
        return Err(Error::new(
            codes::BUILD_002,
            format!("no such cell '{}'", cell.as_deref().unwrap_or_default()),
        ));
    }

    let pipeline = BuildPipeline {
        file: &file,
        output: &output,
        arch: &arch,
        runner: &runner,
        policy,
        cache: cache.as_deref(),
        proxy: proxy.as_deref(),
        offline,
        root_lock: root_lock.as_deref(),
        root: root.as_deref(),
        unsafe_host_root,
        source_date_epoch,
        jobs,
        step_timeout,
        run_tests,
        sbom: sbom.as_deref(),
        test_root_lock: test_root_lock.as_deref(),
        signing: &signing,
        pipeline_dir,
    };
    let built = pipeline.execute(&selected)?;
    report(json, &built.rpms, &built.srpms)
}

fn report(json: bool, rpms: &[PathBuf], srpms: &[PathBuf]) -> Result<Vec<PathBuf>, Error> {
    crate::commands::report::print(
        json,
        "build",
        &[
            crate::commands::report::ArtifactGroup {
                kind: "srpm",
                paths: srpms,
            },
            crate::commands::report::ArtifactGroup {
                kind: "rpm",
                paths: rpms,
            },
        ],
        None,
    )?;
    Ok(rpms.to_vec())
}
