use std::path::{Path, PathBuf};

use crate::commands::emit;
use rpmx_core::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use rpmx_core::build_env::check_build_tools;
use rpmx_core::error::codes;
use rpmx_core::package_spec::BinaryPackageFile;
use rpmx_core::runner::RunnerKind;
use rpmx_core::sandbox::{Sandbox, SandboxConfig};
use rpmx_core::sbom::Generator;
use rpmx_core::test_root::{prepare as prepare_test_root, TestRootOptions};
use rpmx_core::test_runner::{run_planned_tests, TestSandboxConfig};
use rpmx_core::Error;

mod prepare;

use crate::commands::RebuildArgs;
use prepare::{prepare, RebuildState};

pub fn run(args: RebuildArgs<'_>) -> Result<Vec<PathBuf>, Error> {
    let signing = crate::commands::load_signing_key(args.signing_key, args.signing_passphrase_env)?;
    let state = prepare(&args, signing.as_ref())?;
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
    let output = args.output;
    let run_tests = args.run_tests;
    let sbom = args.sbom;
    let quiet = args.quiet;
    let json = args.json;
    let jobs = args.jobs;
    let step_timeout = args.step_timeout;
    let runner = args.runner;
    let arch = args.arch;

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
        image: args.root_image,
        source_date_epoch: extracted.source_date_epoch,
        jobs,
        timeout: Some(step_timeout).filter(|seconds| *seconds > 0),
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

    std::fs::create_dir_all(output).map_err(|error| {
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

    let sbom_policy = sbom_enabled || sbom.is_some();
    let mut sbom_sources: Vec<(String, String)> = Vec::new();
    let mut recipe_digest = String::new();
    if sbom_policy {
        recipe_digest = package_lock.recipe_digest.clone();
        sbom_sources = package_lock
            .sources
            .iter()
            .map(|source| (source.path.clone(), source.digest.clone()))
            .collect();
        if recipe_digest.is_empty() {
            if let Ok(bytes) = std::fs::read(work.path().join("package.yaml")) {
                recipe_digest = rpmx_core::srpm::sha512_hex(&bytes);
            }
        }
    }
    let sbom_refs = sbom_sources
        .iter()
        .map(|(path, digest)| rpmx_core::sbom::SourceRef { path, digest })
        .collect::<Vec<_>>();
    let source_rpm = format!("{}-{}-{}.src.rpm", cell.package, cell.version, cell.release);

    let mut written = Vec::new();
    let mut built = Vec::new();
    for mut package in packages {
        if sbom_policy {
            let text = rpmx_core::sbom::native_spdx().package(&rpmx_core::sbom::PackageSbom {
                name: &package.name,
                version: &package.version,
                release: &package.release,
                arch: &package.arch,
                license: &package.license,
                source_rpm: &source_rpm,
                recipe_digest: &recipe_digest,
                source_date_epoch: package.source_date_epoch,
                sources: &sbom_refs,
                cpe: recipe.package.cpe.as_ref(),
            })?;
            package.files.push(BinaryPackageFile {
                path: format!(
                    "{}/{}-{}-{}.spdx.json",
                    rpmx_core::sbom::SBOM_DIR,
                    package.name,
                    package.version,
                    package.release
                ),
                data: text.into_bytes(),
                mode: 0o644,
                target: None,
                user: None,
                group: None,
                caps: None,
                config: false,
                noreplace: false,
                doc: false,
                license: false,
            });
        }
        let path = emit::binary(&package, output, signing.as_ref(), signature_timestamp)?;
        built.push((
            path.clone(),
            package.name.clone(),
            package.version.clone(),
            package.release.clone(),
            package.arch.clone(),
        ));
        written.push(path);
    }

    if sbom_policy {
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
        let text = rpmx_core::sbom::native_spdx().build(&rpmx_core::sbom::BuildSbom {
            name: &cell.package,
            version: &cell.version,
            release: &cell.release,
            arch,
            license: &recipe.package.license,
            recipe_digest: &recipe_digest,
            source_date_epoch: extracted.source_date_epoch,
            sources: &sbom_refs,
            binaries: &binaries,
            root_lock: active_lock.as_ref(),
            signing_key: signing.as_ref().map(|key| key.fingerprint()),
            cpe: recipe.package.cpe.as_ref(),
        })?;
        let path = sbom.map(Path::to_path_buf).unwrap_or_else(|| {
            output.join(format!(
                "{}-{}-{}.build.spdx.json",
                cell.package, cell.version, cell.release
            ))
        });
        std::fs::write(&path, text).map_err(|error| {
            Error::new(
                codes::SBOM_001,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }

    if run_tests {
        let cache = crate::commands::fetch::default_cache()?;
        let prepared = if cell.tests.iter().any(|test| test.stage == "installed") {
            let lock = active_lock.as_ref().ok_or_else(|| {
                Error::new(
                    codes::BUILD_007,
                    "installed-stage tests need an embedded or explicit root lock",
                )
            })?;
            Some(prepare_test_root(TestRootOptions {
                lock,
                pinned: args.test_root_lock,
                tools: &cell.test_tools,
                rpms: &written,
                image: builder_image.as_deref(),
                cache: &cache,
                work: work.path(),
                runner: runner_kind,
            })?)
        } else {
            None
        };
        let test_root = prepared.as_ref().map(|root| root.path.as_path());
        let result = run_planned_tests(
            &sandbox,
            &cell.tests,
            test_root,
            TestSandboxConfig {
                workspace: &build,
                stage: &stage,
                host_arch: rpmx_core::arch::Arch::current()?,
                target_arch: rpmx_core::arch::Arch::parse(arch)?,
                runner: runner_kind,
                source_date_epoch: extracted.source_date_epoch,
                jobs,
                timeout: Some(step_timeout).filter(|seconds| *seconds > 0),
                debuginfo,
            },
        );
        if let Some(prepared) = prepared {
            let _ = rpmx_core::root::remove_tree(&prepared.path);
            let path = output.join(format!(
                "{}-{}-{}.test-root.lock.json",
                cell.package, cell.version, cell.release
            ));
            let text = serde_yaml_ng::to_string(&prepared.lock)
                .map_err(|error| Error::new(codes::BUILD_007, error.to_string()))?;
            std::fs::write(&path, text).map_err(|error| {
                Error::new(
                    codes::BUILD_007,
                    format!("cannot write {}: {error}", path.display()),
                )
            })?;
        }
        result?;
    }

    if !quiet {
        crate::commands::report::print(
            json,
            &[crate::commands::report::ArtifactGroup {
                key: "rpms",
                paths: &written,
            }],
            signing.as_ref().map(|key| key.fingerprint()),
            true,
        )?;
    }
    Ok(written)
}
