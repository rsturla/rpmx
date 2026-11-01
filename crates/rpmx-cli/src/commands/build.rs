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
    let profile = args.policy.profile.clone();
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| crate::commands::default_output(&args.arch));
    if let Some(stage) = args.stage.as_deref() {
        return package_stage(&args, stage, &output, pipeline_dir, policy);
    }
    build_from_recipe(args, &output, pipeline_dir, policy, profile.as_deref())
}

/// Package an already-staged tree. This is the original `build --stage` mode.
fn package_stage(
    args: &BuildArgs,
    stage: &Path,
    output: &Path,
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
            arch: &args.arch,
            source_date_epoch,
            debuginfo: false,
        })?;
        if !args.no_lint {
            let findings = crate::commands::package_lint::check(&packages);
            crate::commands::package_lint::enforce(&findings)?;
        }
        for package in packages {
            written.push(emit::binary(
                &package,
                output,
                signing.as_ref(),
                signature_timestamp,
            )?);
        }
    }

    report(args.json, &written, &[], &[])?;
    if !args.no_index {
        crate::commands::repo::index(output, signing.as_ref(), source_date_epoch)?;
    }
    Ok(written)
}

/// Run the full SRPM-first pipeline: source snapshot, SRPM, binary RPMs.
fn build_from_recipe(
    args: BuildArgs,
    output: &Path,
    pipeline_dir: Option<&Path>,
    policy: PlanPolicy<'_>,
    profile: Option<&Path>,
) -> Result<Vec<PathBuf>, Error> {
    let BuildArgs {
        file,
        output: _,
        arch,
        runner,
        source_date_epoch,
        cell,
        cache,
        proxy,
        offline,
        locked,
        root_lock,
        root,
        unsafe_host_root,
        jobs,
        step_timeout,
        run_tests,
        test,
        junit,
        no_index,
        no_lint,
        sbom,
        test_root_lock,
        signing,
        json,
        ..
    } = args;

    let run_tests = run_tests || !test.is_empty();
    let (recipe, plan) = load_plan(&file, &arch, pipeline_dir, false, policy)?;
    if locked {
        crate::commands::lock::verify(&recipe, &plan, &file, policy, profile, &arch)?;
    }
    let automatic = if root_lock.is_none() && root.is_none() && !unsafe_host_root {
        crate::commands::autoroot::ensure(
            &recipe,
            &plan,
            &file,
            profile,
            cache.as_deref(),
            offline,
            false,
            &arch,
        )?
    } else {
        None
    };
    let verify_digest = automatic.is_none();
    let (root_lock, root) = match automatic {
        Some(root) => {
            eprintln!(
                "root {} x86_64 {} packages {} ({})",
                root.profile,
                root.packages,
                root.document.root_digest.as_deref().unwrap_or("-"),
                if root.cached { "cache" } else { "resolved" }
            );
            (Some(root.lock), Some(root.root))
        }
        None => (root_lock, root),
    };
    let _root_request = rpmx_core::request::resolve(rpmx_core::request::RootRequestOptions {
        lock: root_lock.as_deref(),
        root: root.as_deref(),
        allow_host: unsafe_host_root,
        arch: &arch,
        verify_digest,
    })?;

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
        output,
        arch: &arch,
        runner: &runner,
        policy,
        profile,
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
        test_filter: &test,
        junit: junit.as_deref(),
        no_lint,
        sbom: sbom.as_deref(),
        test_root_lock: test_root_lock.as_deref(),
        signing: &signing,
        pipeline_dir,
    };
    let built = pipeline.execute(&selected)?;
    report(json, &built.rpms, &built.srpms, &built.evidence)?;
    if !no_index {
        let timestamp = crate::commands::source_date_epoch(&recipe, source_date_epoch);
        let key = crate::commands::load_signing_key(
            signing.signing_key.as_deref(),
            signing.signing_passphrase_env.as_deref(),
        )?;
        crate::commands::repo::index(output, key.as_ref(), timestamp)?;
    }
    Ok(built.rpms)
}

fn report(
    json: bool,
    rpms: &[PathBuf],
    srpms: &[PathBuf],
    evidence: &[PathBuf],
) -> Result<Vec<PathBuf>, Error> {
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
            crate::commands::report::ArtifactGroup {
                kind: "evidence",
                paths: evidence,
            },
        ],
        None,
    )?;
    Ok(rpms.to_vec())
}
