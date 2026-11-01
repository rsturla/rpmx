use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

use crate::commands::emit;
use rpmx_core::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use rpmx_core::build_env::check_build_tools;
use rpmx_core::error::codes;
use rpmx_core::runner::RunnerKind;
use rpmx_core::sandbox::{Sandbox, SandboxConfig};
use rpmx_core::Error;

mod prepare;
mod report;
mod sbom;
mod tests;

use crate::commands::{OptionalSigningArgs, PlanPolicy, RebuildArgs};
use prepare::{prepare, RebuildState};
use sbom::SbomContext;

/// Everything rebuilding an SRPM needs, independent of clap.
pub struct RebuildRequest<'a> {
    pub file: &'a Path,
    pub output: &'a Path,
    pub arch: &'a str,
    pub runner: &'a str,
    pub cell: Option<&'a str>,
    pub policy: PlanPolicy<'a>,
    pub jobs: NonZeroU32,
    pub step_timeout: NonZeroU64,
    pub root_image: Option<&'a str>,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub cache: Option<&'a Path>,
    pub offline: bool,
    /// Re-hash an explicit root against its lock. A full build that just
    /// resolved or cached the root passes false.
    pub verify_root: bool,
    pub run_tests: bool,
    pub test_filter: &'a [String],
    pub no_index: bool,
    pub no_lint: bool,
    pub junit: Option<&'a Path>,
    pub sbom: Option<&'a Path>,
    pub test_root_lock: Option<&'a Path>,
    pub signing: &'a OptionalSigningArgs,
    pub quiet: bool,
}

pub struct RebuildOutput {
    pub rpms: Vec<PathBuf>,
    pub evidence: Vec<PathBuf>,
}

pub fn run(args: RebuildArgs) -> Result<Vec<PathBuf>, Error> {
    if let Some(path) = args.junit.as_deref() {
        let _ = std::fs::remove_file(path);
    }
    let resolved = crate::commands::resolve_policy(crate::commands::PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| crate::commands::default_output(&args.arch));
    execute(RebuildRequest {
        file: &args.file,
        output: &output,
        arch: &args.arch,
        runner: &args.runner,
        cell: args.cell.as_deref(),
        policy: resolved.plan_policy(),
        jobs: args.runtime.jobs,
        step_timeout: args.runtime.step_timeout,
        root_image: args.root_image.as_deref(),
        root_lock: args.root_lock.as_deref(),
        root: args.root.as_deref(),
        cache: args.runtime.cache.cache.as_deref(),
        offline: args.offline,
        verify_root: true,
        run_tests: args.run_tests || !args.test.is_empty(),
        test_filter: &args.test,
        no_index: args.no_index,
        no_lint: args.no_lint,
        junit: args.junit.as_deref(),
        sbom: args.sbom.as_deref(),
        test_root_lock: args.test_root_lock.as_deref(),
        signing: &args.signing,
        quiet: args.quiet,
    })
    .map(|output| output.rpms)
}

pub fn execute(request: RebuildRequest<'_>) -> Result<RebuildOutput, Error> {
    let signing = crate::commands::load_signing_key(
        request.signing.signing_key.as_deref(),
        request.signing.signing_passphrase_env.as_deref(),
    )?;
    let state = prepare(&request, signing.as_ref())?;
    let RebuildState {
        work,
        extracted,
        package_lock,
        recipe,
        cell,
        build,
        stage,
        active_lock,
        builder_image,
        locked_packages,
        assembled_root,
        signature_timestamp,
    } = state;
    let output = request.output.to_path_buf();
    let run_tests = request.run_tests;
    let sbom_path = request.sbom;
    let quiet = request.quiet;
    let json = rpmx_core::output::json();
    let jobs = request.jobs;
    let step_timeout = request.step_timeout;
    let runner = request.runner;
    let arch = request.arch;

    let host_arch = rpmx_core::arch::Arch::current()?;
    let target_arch = rpmx_core::arch::Arch::parse(arch)?;
    let runner_kind = runner.parse::<RunnerKind>()?;
    let debuginfo = active_lock.as_ref().is_some_and(|lock| lock.debuginfo);
    let sbom_enabled = active_lock.as_ref().map(|lock| lock.sbom).unwrap_or(true);
    let prefix = active_lock
        .as_ref()
        .map(|lock| lock.prefix.as_str())
        .unwrap_or(rpmx_core::guest::PREFIX);
    let libdir = active_lock
        .as_ref()
        .map(|lock| lock.libdir.as_str())
        .unwrap_or(rpmx_core::guest::LIBDIR);
    let sandbox = Sandbox::new(SandboxConfig {
        runner: runner_kind,
        host: host_arch,
        target: target_arch,
        workspace: &build,
        stage: &stage,
        root: assembled_root.as_deref(),
        image: request.root_image,
        prefix,
        libdir,
        source_date_epoch: extracted.source_date_epoch,
        jobs,
        timeout: step_timeout,
        debuginfo,
    })?;

    check_build_tools(
        &sandbox,
        &build,
        &cell.tools,
        &cell.host,
        &cell.target,
        &cell.test_tools,
        &locked_packages,
    )?;
    for step in &cell.pipeline {
        if let Some(digest) = &step.step_digest {
            match extracted.lock_steps.get(&step.uses) {
                Some(expected) if expected == digest => {}
                Some(expected) => {
                    return Err(Error::new(
                        codes::SRPM_011,
                        format!(
                            "step '{}' digest {digest} does not match lock {expected}",
                            step.uses
                        ),
                    ));
                }
                None => {
                    return Err(Error::new(
                        codes::SRPM_011,
                        format!("step '{}' is missing from the lock", step.uses),
                    ));
                }
            }
        }
        sandbox.run(step)?;
    }

    std::fs::create_dir_all(&output).map_err(|error| {
        Error::new(
            codes::BUILD_001,
            format!("cannot create {}: {error}", output.display()),
        )
    })?;

    sandbox.sync()?;

    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &cell,
        stage: &stage,
        arch,
        source_date_epoch: extracted.source_date_epoch,
        debuginfo,
    })?;

    if !request.no_lint {
        let findings = crate::commands::package_lint::check(&packages);
        crate::commands::package_lint::enforce(&findings)?;
    }

    let sbom_policy = sbom_enabled || sbom_path.is_some();
    let sbom = if sbom_policy {
        Some(SbomContext::prepare(&package_lock, work.path(), &cell))
    } else {
        None
    };

    let mut written = Vec::new();
    let mut evidence = Vec::new();
    let mut built = Vec::new();
    for mut package in packages {
        if let Some(sbom) = &sbom {
            sbom.attach_package(&mut package, &recipe)?;
        }
        let path = emit::binary(&package, &output, signing.as_ref(), signature_timestamp)?;
        built.push((
            path.clone(),
            package.name.clone(),
            package.version.clone(),
            package.release.clone(),
            package.arch.clone(),
        ));
        written.push(path);
    }

    if let Some(sbom) = &sbom {
        let binaries = built
            .iter()
            .map(
                |(path, name, version, release, arch)| rpmx_core::sbom::BinaryRef {
                    name,
                    version,
                    release,
                    arch,
                    license: &recipe.package.license,
                    path,
                },
            )
            .collect::<Vec<_>>();
        let path = sbom_path
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| {
                output.join(format!(
                    "{}-{}-{}.build.spdx.json",
                    cell.package, cell.version, cell.release
                ))
            });
        sbom.emit_build(sbom::BuildSbomInput {
            recipe: &recipe,
            cell: &cell,
            arch,
            source_date_epoch: extracted.source_date_epoch,
            binaries: &binaries,
            root_lock: active_lock.as_ref(),
            signing_key: signing.as_ref().map(|key| key.fingerprint()),
            path: &path,
        })?;
        evidence.push(path);
    }

    if run_tests {
        if let Some(path) = tests::run(tests::TestRunOptions {
            sandbox: &sandbox,
            cell: &cell,
            written: &written,
            output: &output,
            work: work.path(),
            build: &build,
            stage: &stage,
            active_lock: active_lock.as_ref(),
            builder_image: builder_image.as_deref(),
            pinned_lock: request.test_root_lock,
            arch,
            runner: runner_kind,
            source_date_epoch: extracted.source_date_epoch,
            jobs,
            timeout: step_timeout,
            debuginfo,
            filter: request.test_filter,
            junit: request.junit,
        })? {
            evidence.push(path);
        }
    }

    let mut repository = Vec::new();
    if !request.no_index {
        let report = crate::commands::repo::index(
            &output,
            signing.as_ref(),
            signature_timestamp.unwrap_or(extracted.source_date_epoch),
        )?;
        repository.push(report.repomd);
        if let Some(signature) = report.signature {
            repository.push(signature);
        }
    }
    report::print_artifacts(
        quiet,
        json,
        &written,
        &evidence,
        &repository,
        signing.as_ref().map(|key| key.fingerprint()),
    )?;
    Ok(RebuildOutput {
        rpms: written,
        evidence,
    })
}
