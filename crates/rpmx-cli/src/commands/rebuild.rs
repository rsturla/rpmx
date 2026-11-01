use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::commands::emit;
use rpmx_core::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use rpmx_core::build_env::check_build_tools;
use rpmx_core::error::codes;
use rpmx_core::package_spec::BinaryPackageFile;
use rpmx_core::root::{load_lock, RootSelection};
use rpmx_core::sandbox::{RunnerKind, Sandbox};
use rpmx_core::sbom::Generator;
use rpmx_core::srpm::load_lock_file;
use rpmx_core::steps::validate_name;
use rpmx_core::test_root::{prepare as prepare_test_root, TestRootOptions};
use rpmx_core::test_runner::{run_planned_tests, TestSandboxConfig};
use rpmx_core::Error;
use rpmx_rpm::extract::extract_srpm;

use crate::commands::{load_plan, reconcile_policy, PlanPolicy, RebuildArgs};

pub fn run(args: RebuildArgs<'_>) -> Result<Vec<PathBuf>, Error> {
    let RebuildArgs {
        file,
        output,
        arch,
        runner,
        cell,
        policy,
        jobs,
        step_timeout,
        root_image,
        root_lock,
        root,
        run_tests,
        sbom,
        signing_key,
        signing_passphrase_env,
        signing_timestamp,
        test_root_lock,
        quiet,
        json,
    } = args;
    let signing = crate::commands::load_signing_key(signing_key, signing_passphrase_env)?;

    let work = tempfile::tempdir().map_err(|error| {
        Error::new(
            codes::BUILD_005,
            format!("cannot create work directory: {error}"),
        )
    })?;
    let extracted = extract_srpm(file, work.path())?;
    // Read the authoritative lock before unpacking the snapshot. The snapshot
    // is user-controlled recipe data and must not be able to replace it.
    let package_lock = load_lock_file(&work.path().join("package.lock.json"))?;
    let signature_timestamp = crate::commands::signature_timestamp(
        signing_timestamp,
        extracted.source_date_epoch,
        signing.as_ref(),
    )?;
    let build = work.path().join("build");
    std::fs::create_dir_all(&build).map_err(|error| {
        Error::new(
            codes::BUILD_005,
            format!("cannot create {}: {error}", build.display()),
        )
    })?;
    let snapshot = work.path().join("RPMX-SOURCE.tar.gz");
    if snapshot.exists() {
        rpmx_core::source::unpack_snapshot(&snapshot, &build)?;
    }

    let pipelines = work.path().join("pipelines");
    for (name, content) in &extracted.steps {
        // Built-in steps may be single-segment (`make`, `cmake`), while local
        // pipeline files must be namespaced. The digest check below still
        // pins their content.
        if rpmx_core::steps::builtin_source(name).is_none() {
            validate_name(name)?;
        }
        let path = pipelines.join(format!("{name}.yaml"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::new(
                    codes::BUILD_006,
                    format!("cannot create {}: {error}", parent.display()),
                )
            })?;
        }
        std::fs::write(&path, content).map_err(|error| {
            Error::new(
                codes::BUILD_006,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }

    let locked_policy = PlanPolicy {
        dist_tag: package_lock.dist_tag.as_deref(),
        build_release: package_lock.build_release,
    };
    let policy = reconcile_policy(policy, locked_policy)?;

    let recipe_path = work.path().join("package.yaml");
    let (recipe, plan) = load_plan(&recipe_path, arch, Some(pipelines.as_path()), true, policy)?;
    let cells: Vec<_> = match cell {
        Some(id) => plan.cells.iter().filter(|cell| cell.id == id).collect(),
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
            "this plan has several cells; pass --cell for each rebuild",
        ));
    }
    if cells[0].version != package_lock.version || cells[0].release != package_lock.release {
        return Err(Error::new(
            codes::SRPM_012,
            format!(
                "reconstructed {}-{}-{} does not match the SRPM lock's {}-{}-{}",
                cells[0].package,
                cells[0].version,
                cells[0].release,
                cells[0].package,
                package_lock.version,
                package_lock.release
            ),
        ));
    }

    let stage = work.path().join("stage");
    std::fs::create_dir_all(&stage).map_err(|error| {
        Error::new(
            codes::BUILD_006,
            format!("cannot create {}: {error}", stage.display()),
        )
    })?;

    let mut locked_packages: BTreeSet<String> = BTreeSet::new();
    let mut builder_image: Option<String> = root_image.map(str::to_string);
    let embedded_lock = {
        let path = work.path().join("root.lock.json");
        if path.exists() {
            Some(load_lock(&path)?)
        } else {
            None
        }
    };
    let requested_lock = match root_lock {
        Some(path) => Some(load_lock(path)?),
        None => None,
    };
    if let (Some(requested), Some(embedded)) = (&requested_lock, &embedded_lock) {
        if requested.closure_digest != embedded.closure_digest
            || requested.root_digest != embedded.root_digest
            || requested.debuginfo != embedded.debuginfo
        {
            return Err(Error::new(
                codes::ROOT_016,
                "--root-lock differs from the root lock embedded in the SRPM \
                 (closure or root digest)",
            ));
        }
    }
    let active_lock = requested_lock.or(embedded_lock);

    let assembled_root: Option<PathBuf> = if let Some(lock) = &active_lock {
        if root_image.is_some() {
            return Err(Error::new(
                codes::ROOT_016,
                "a root lock cannot be combined with --root-image",
            ));
        }
        let root_path = root.ok_or_else(|| {
            Error::new(
                codes::ROOT_016,
                "a root lock is available; pass --root with the assembled root directory",
            )
        })?;
        builder_image = lock.builder.as_ref().map(|builder| builder.image.clone());
        for entries in lock.packages.values() {
            for package in entries {
                locked_packages.insert(package.name.clone());
            }
        }
        let selection = RootSelection::verify(lock, root_path, arch)?;
        Some(selection.path)
    } else {
        if root.is_some() {
            return Err(Error::new(
                codes::ROOT_016,
                "--root requires a root lock: pass --root-lock or use an SRPM with root.lock.json",
            ));
        }
        if root_image.is_none() {
            return Err(Error::new(
                codes::ROOT_016,
                "no root available: the SRPM has no root.lock.json and --root-lock was not given; \
                 pass --root-lock, or --root-image for the image backend",
            ));
        }
        None
    };

    let host_arch = rpmx_core::arch::Arch::current()?;
    let target_arch = rpmx_core::arch::Arch::parse(arch)?;
    let runner_kind = runner.parse::<RunnerKind>()?;
    let backend = runner_kind.select(
        host_arch,
        target_arch,
        assembled_root.as_deref(),
        root_image,
    )?;

    let debuginfo = active_lock.as_ref().is_some_and(|lock| lock.debuginfo);
    let sbom_enabled = active_lock.as_ref().map(|lock| lock.sbom).unwrap_or(true);
    let sandbox = Sandbox {
        backend,
        workspace: &build,
        stage: &stage,
        host_arch,
        target_arch,
        source_date_epoch: extracted.source_date_epoch,
        jobs,
        timeout: Some(step_timeout).filter(|seconds| *seconds > 0),
        debuginfo,
        session: std::cell::RefCell::new(None),
    };

    check_build_tools(
        &sandbox,
        &build,
        &cells[0].tools,
        &cells[0].host,
        &cells[0].target,
        &cells[0].test_tools,
        &locked_packages,
    )?;
    for step in &cells[0].pipeline {
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
        cell: cells[0],
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
    let source_rpm = format!(
        "{}-{}-{}.src.rpm",
        cells[0].package, cells[0].version, cells[0].release
    );

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
            name: &cells[0].package,
            version: &cells[0].version,
            release: &cells[0].release,
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
                cells[0].package, cells[0].version, cells[0].release
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
        let prepared = if cells[0].tests.iter().any(|test| test.stage == "installed") {
            let lock = active_lock.as_ref().ok_or_else(|| {
                Error::new(
                    codes::BUILD_007,
                    "installed-stage tests need an embedded or explicit root lock",
                )
            })?;
            Some(prepare_test_root(TestRootOptions {
                lock,
                pinned: test_root_lock,
                tools: &cells[0].test_tools,
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
            &cells[0].tests,
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
                cells[0].package, cells[0].version, cells[0].release
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
        if json {
            let value = serde_json::json!({
                "ok": true,
                "rpms": written.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
                "signing_key": signing.as_ref().map(|key| key.fingerprint()),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&value)
                    .map_err(|error| Error::new(codes::CLI_005, error.to_string()))?
            );
        } else {
            for path in &written {
                println!("{}", path.display());
            }
        }
    }
    Ok(written)
}
