use std::path::{Path, PathBuf};

use crate::commands::emit;
use rpmx_core::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{
    load_plan, rebuild, resolve_policy, source, srpm, BuildArgs, PlanPolicy, PolicyInput,
    RebuildArgs, SourceArgs, SrpmArgs,
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

    if unsafe_host_root && (root_lock.is_some() || root.is_some()) {
        return Err(Error::new(
            codes::ROOT_016,
            "--unsafe-host-root cannot be combined with --root-lock or --root",
        ));
    }
    if !unsafe_host_root && (root_lock.is_none() || root.is_none()) {
        return Err(Error::new(
            codes::ROOT_016,
            "a full build requires --root-lock and --root for source assembly; \
             pass --unsafe-host-root to use the host root",
        ));
    }

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

    std::fs::create_dir_all(&output).map_err(|error| {
        Error::new(
            codes::BUILD_001,
            format!("cannot create {}: {error}", output.display()),
        )
    })?;
    let srpm_output = output.join("src");
    let work = tempfile::tempdir().map_err(|error| {
        Error::new(
            codes::BUILD_005,
            format!("cannot create work directory: {error}"),
        )
    })?;
    let source_output = work.path().join("source");
    std::fs::create_dir_all(&source_output).map_err(|error| {
        Error::new(
            codes::BUILD_006,
            format!("cannot create {}: {error}", source_output.display()),
        )
    })?;

    source::run(
        SourceArgs {
            file: file.clone(),
            arch: arch.clone(),
            runner: runner.clone(),
            policy: crate::cli::PolicyArgs {
                profile: None,
                dist_tag: policy.dist_tag.map(str::to_string),
                build_release: policy.build_release,
            },
            cache: cache.clone(),
            proxy: proxy.clone(),
            offline,
            root_lock: root_lock.clone(),
            root: root.clone(),
            unsafe_host_root,
            output: source_output.clone(),
            source_date_epoch,
            jobs,
            step_timeout,
            json: false,
            quiet: true,
        },
        pipeline_dir,
    )?;

    let mut srpms = Vec::new();
    let mut rpms = Vec::new();
    for planned in &selected {
        let written = srpm::run(
            SrpmArgs {
                file: file.clone(),
                policy: crate::cli::PolicyArgs {
                    profile: None,
                    dist_tag: policy.dist_tag.map(str::to_string),
                    build_release: policy.build_release,
                },
                sources: source_output.clone(),
                output: srpm_output.clone(),
                arch: arch.clone(),
                source_date_epoch,
                cell: Some(planned.id.clone()),
                root_lock: root_lock.clone(),
                signing: signing.clone(),
                json: false,
                quiet: true,
            },
            pipeline_dir,
        )?;
        srpms.extend(written.iter().cloned());

        for srpm in written {
            rpms.extend(rebuild::run(RebuildArgs {
                file: srpm.clone(),
                output: output.clone(),
                arch: arch.clone(),
                runner: runner.clone(),
                cell: Some(planned.id.clone()),
                policy: crate::cli::PolicyArgs {
                    profile: None,
                    dist_tag: policy.dist_tag.map(str::to_string),
                    build_release: policy.build_release,
                },
                jobs,
                step_timeout,
                root_image: None,
                root_lock: root_lock.clone(),
                root: root.clone(),
                run_tests,
                sbom: sbom.clone(),
                test_root_lock: test_root_lock.clone(),
                signing: signing.clone(),
                json: false,
                quiet: true,
            })?);
        }
    }

    report(json, &rpms, &srpms)
}

fn report(json: bool, rpms: &[PathBuf], srpms: &[PathBuf]) -> Result<Vec<PathBuf>, Error> {
    crate::commands::report::print(
        json,
        &[
            crate::commands::report::ArtifactGroup {
                key: "srpms",
                paths: srpms,
            },
            crate::commands::report::ArtifactGroup {
                key: "rpms",
                paths: rpms,
            },
        ],
        None,
        false,
    )?;
    Ok(rpms.to_vec())
}
