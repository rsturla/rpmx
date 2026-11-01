use std::path::PathBuf;

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

use crate::commands::RebuildArgs;
use prepare::{prepare, RebuildState};
use sbom::SbomContext;

pub fn run(args: RebuildArgs) -> Result<Vec<PathBuf>, Error> {
    let resolved = crate::commands::resolve_policy(crate::commands::PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    let signing = crate::commands::load_signing_key(
        args.signing.signing_key.as_deref(),
        args.signing.signing_passphrase_env.as_deref(),
    )?;
    let state = prepare(&args, signing.as_ref(), policy)?;
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
    let output = args.output.clone();
    let run_tests = args.run_tests;
    let sbom_path = args.sbom.as_deref();
    let quiet = args.quiet;
    let json = args.json;
    let jobs = args.jobs;
    let step_timeout = args.step_timeout;
    let runner = args.runner.as_str();
    let arch = args.arch.as_str();

    let host_arch = rpmx_core::arch::Arch::current()?;
    let target_arch = rpmx_core::arch::Arch::parse(arch)?;
    let runner_kind = runner.parse::<RunnerKind>()?;
    let debuginfo = active_lock.as_ref().is_some_and(|lock| lock.debuginfo);
    let sbom_enabled = active_lock.as_ref().map(|lock| lock.sbom).unwrap_or(true);
    let sandbox = Sandbox::new(SandboxConfig {
        runner: runner_kind,
        host: host_arch,
        target: target_arch,
        workspace: &build,
        stage: &stage,
        root: assembled_root.as_deref(),
        image: args.root_image.as_deref(),
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

    let sbom_policy = sbom_enabled || sbom_path.is_some();
    let sbom = if sbom_policy {
        Some(SbomContext::prepare(&package_lock, work.path(), &cell))
    } else {
        None
    };

    let mut written = Vec::new();
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
    }

    if run_tests {
        tests::run(tests::TestRunOptions {
            sandbox: &sandbox,
            cell: &cell,
            written: &written,
            output: &output,
            work: work.path(),
            build: &build,
            stage: &stage,
            active_lock: active_lock.as_ref(),
            builder_image: builder_image.as_deref(),
            pinned_lock: args.test_root_lock.as_deref(),
            arch,
            runner: runner_kind,
            source_date_epoch: extracted.source_date_epoch,
            jobs,
            timeout: step_timeout,
            debuginfo,
        })?;
    }

    report::print_artifacts(
        quiet,
        json,
        &written,
        signing.as_ref().map(|key| key.fingerprint()),
    )?;
    Ok(written)
}
