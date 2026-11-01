use std::path::{Path, PathBuf};

use crate::commands::emit;
use rpmx_core::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{
    load_plan, rebuild, source, srpm, BuildArgs, RebuildArgs, SourceArgs, SrpmArgs,
};

pub fn run(args: BuildArgs<'_>) -> Result<Vec<PathBuf>, Error> {
    if let Some(stage) = args.stage {
        return package_stage(&args, stage);
    }
    build_from_recipe(args)
}

/// Package an already-staged tree. This is the original `build --stage` mode.
fn package_stage(args: &BuildArgs<'_>, stage: &Path) -> Result<Vec<PathBuf>, Error> {
    let BuildArgs {
        file,
        output,
        arch,
        source_date_epoch,
        cell,
        pipeline_dir,
        policy,
        signing_key,
        signing_passphrase_env,
        signing_timestamp,
        json,
        ..
    } = args;
    let signing = crate::commands::load_signing_key(*signing_key, *signing_passphrase_env)?;
    let (recipe, plan) = load_plan(file, arch, *pipeline_dir, false, *policy)?;
    let source_date_epoch = crate::commands::source_date_epoch(&recipe, *source_date_epoch);
    let signature_timestamp = crate::commands::signature_timestamp(
        *signing_timestamp,
        source_date_epoch,
        signing.as_ref(),
    )?;
    let cells: Vec<_> = match cell {
        Some(id) => plan.cells.iter().filter(|cell| cell.id == *id).collect(),
        None => plan.cells.iter().collect(),
    };
    if cells.is_empty() {
        return Err(Error::new(
            codes::BUILD_002,
            format!("no such cell '{}'", cell.unwrap_or_default()),
        ));
    }
    if cells.len() > 1 {
        return Err(Error::new(
            codes::BUILD_003,
            "this plan has several cells; pass --cell for each per-cell stage",
        ));
    }

    std::fs::create_dir_all(output).map_err(|error| {
        Error::new(
            codes::BUILD_001,
            format!("cannot create {}: {error}", output.display()),
        )
    })?;

    let mut written = Vec::new();
    for cell in &cells {
        let packages = prepare_binary_packages(BinaryPackageOptions {
            recipe: &recipe,
            cell,
            stage,
            arch,
            source_date_epoch,
            debuginfo: false,
        })?;
        for package in packages {
            written.push(emit::binary(
                &package,
                output,
                signing.as_ref(),
                signature_timestamp,
            )?);
        }
    }

    report(*json, &written, &[])
}

/// Run the full SRPM-first pipeline: source snapshot, SRPM, binary RPMs.
fn build_from_recipe(args: BuildArgs<'_>) -> Result<Vec<PathBuf>, Error> {
    let BuildArgs {
        file,
        output,
        arch,
        runner,
        source_date_epoch,
        cell,
        pipeline_dir,
        policy,
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
        signing_key,
        signing_passphrase_env,
        signing_timestamp,
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

    let (_, plan) = load_plan(file, arch, pipeline_dir, false, policy)?;
    let selected: Vec<_> = match cell {
        Some(id) => plan.cells.iter().filter(|cell| cell.id == id).collect(),
        None => plan.cells.iter().collect(),
    };
    if selected.is_empty() {
        return Err(Error::new(
            codes::BUILD_002,
            format!("no such cell '{}'", cell.unwrap_or_default()),
        ));
    }

    std::fs::create_dir_all(output).map_err(|error| {
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

    source::run(SourceArgs {
        file,
        pipeline_dir,
        policy,
        cache,
        proxy,
        offline,
        output: &source_output,
        arch,
        runner,
        source_date_epoch,
        jobs,
        step_timeout,
        root_lock,
        root,
        unsafe_host_root,
        quiet: true,
        json: false,
    })?;

    let mut srpms = Vec::new();
    let mut rpms = Vec::new();
    for planned in &selected {
        let written = srpm::run(SrpmArgs {
            file,
            sources: &source_output,
            output: &srpm_output,
            arch,
            source_date_epoch,
            cell: Some(planned.id.as_str()),
            pipeline_dir,
            policy,
            root_lock,
            signing_key,
            signing_passphrase_env,
            signing_timestamp,
            quiet: true,
            json: false,
        })?;
        srpms.extend(written.iter().cloned());

        for srpm in written {
            rpms.extend(rebuild::run(RebuildArgs {
                file: &srpm,
                output,
                arch,
                runner,
                cell: Some(planned.id.as_str()),
                policy,
                jobs,
                step_timeout,
                root_image: None,
                root_lock,
                root,
                run_tests,
                sbom,
                test_root_lock,
                signing_key,
                signing_passphrase_env,
                signing_timestamp,
                quiet: true,
                json: false,
            })?);
        }
    }

    report(json, &rpms, &srpms)
}

fn report(json: bool, rpms: &[PathBuf], srpms: &[PathBuf]) -> Result<Vec<PathBuf>, Error> {
    if json {
        let value = serde_json::json!({
            "ok": true,
            "srpms": srpms.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
            "rpms": rpms.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_004, error.to_string()))?
        );
    } else {
        for path in srpms.iter().chain(rpms) {
            println!("{}", path.display());
        }
    }
    Ok(rpms.to_vec())
}
