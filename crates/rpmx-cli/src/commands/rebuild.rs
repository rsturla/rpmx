use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rpmx_core::binary_package::{
    prepare_binary_packages, BinaryPackageOptions, PreparedBinaryFile,
};
use rpmx_core::error::codes;
use rpmx_core::plan::PlannedStep;
use rpmx_core::profile::normalize_arch;
use rpmx_core::root::load_lock;
use rpmx_core::sandbox::{Sandbox, SandboxBackend};
use rpmx_core::sbom::Generator;
use rpmx_core::srpm::load_lock_file;
use rpmx_core::steps::validate_name;
use rpmx_core::Error;
use rpmx_rpm::extract::extract_srpm;
use rpmx_rpm::{write_binary_rpm, BinaryDirectory, BinaryFile, BinarySpec};

use crate::commands::{digest_root, load_plan, reconcile_policy, PlanPolicy, RebuildArgs};

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
        let expected = lock.root_digest.as_deref().ok_or_else(|| {
            Error::new(
                codes::ROOT_016,
                "the root lock has no root_digest; run `rpmx root assemble` first",
            )
        })?;
        let wanted = normalize_arch(arch)?;
        let matches = lock
            .archs
            .iter()
            .any(|candidate| normalize_arch(candidate).ok() == Some(wanted));
        if !matches {
            return Err(Error::new(
                codes::ROOT_016,
                format!(
                    "root lock is for {}, but --arch is {wanted}",
                    lock.archs.join(", ")
                ),
            ));
        }
        let root_path = std::fs::canonicalize(root_path).map_err(|error| {
            Error::new(
                codes::ROOT_017,
                format!("cannot resolve {}: {error}", root_path.display()),
            )
        })?;
        let actual = digest_root(&root_path)?;
        if actual != expected {
            return Err(Error::new(
                codes::ROOT_017,
                format!(
                    "root digest mismatch: lock records {expected}, {} is {actual}",
                    root_path.display()
                ),
            ));
        }
        Some(root_path)
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
    let backend = match runner {
        "qemu" => {
            let root_path = assembled_root.as_deref().ok_or_else(|| {
                Error::new(codes::ROOT_016, "the qemu runner needs an assembled root")
            })?;
            SandboxBackend::Qemu(root_path)
        }
        "bwrap" => {
            let root_path = assembled_root.as_deref().ok_or_else(|| {
                Error::new(
                    codes::ROOT_016,
                    "the bubblewrap runner needs an assembled root",
                )
            })?;
            if host_arch != target_arch {
                return Err(Error::new(
                    codes::RUNNER_002,
                    "bubblewrap cannot run a foreign target; use --runner qemu",
                ));
            }
            SandboxBackend::Root(root_path)
        }
        _ => {
            if let Some(root_path) = assembled_root.as_deref() {
                if host_arch == target_arch {
                    SandboxBackend::Root(root_path)
                } else {
                    SandboxBackend::Qemu(root_path)
                }
            } else if let Some(image) = root_image {
                SandboxBackend::Container(image)
            } else {
                unreachable!("root selection checked above")
            }
        }
    };

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
        let path = output.join(format!(
            "{}-{}-{}.{}.rpm",
            package.name, package.version, package.release, package.arch
        ));
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
            package.files.push(PreparedBinaryFile {
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
        built.push((
            path.clone(),
            package.name.clone(),
            package.version.clone(),
            package.release.clone(),
            package.arch.clone(),
        ));
        let spec = BinarySpec {
            name: package.name,
            version: package.version,
            release: package.release,
            epoch: package.epoch,
            arch: package.arch,
            summary: package.summary,
            license: package.license,
            description: package.description,
            url: package.url,
            source_rpm: package.source_rpm,
            source_date_epoch: package.source_date_epoch,
            files: package
                .files
                .into_iter()
                .map(|file| BinaryFile {
                    path: file.path,
                    data: file.data,
                    mode: file.mode,
                    target: file.target,
                    user: file.user,
                    group: file.group,
                    caps: file.caps,
                    config: file.config,
                    noreplace: file.noreplace,
                    doc: file.doc,
                    license: file.license,
                })
                .collect(),
            directories: package
                .directories
                .into_iter()
                .map(|directory| BinaryDirectory {
                    path: directory.path,
                    mode: directory.mode,
                    user: directory.user,
                    group: directory.group,
                })
                .collect(),
            requires: package.requires,
            provides: package.provides,
            obsoletes: package.obsoletes,
            conflicts: package.conflicts,
            scriptlets: package.scriptlets,
        };
        write_binary_rpm(&spec, &path)?;
        if let Some(key) = &signing {
            rpmx_rpm::sign::sign_package(&path, key, signature_timestamp)?;
        }
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
        let prepared = if cells[0].tests.iter().any(|test| test.stage == "installed") {
            let lock = active_lock.as_ref().ok_or_else(|| {
                Error::new(
                    codes::BUILD_007,
                    "installed-stage tests need an embedded or explicit root lock",
                )
            })?;
            Some(prepare_test_root(
                lock,
                test_root_lock,
                &cells[0].test_tools,
                &written,
                builder_image.as_deref(),
                work.path(),
                runner,
            )?)
        } else {
            None
        };
        let test_root = prepared.as_ref().map(|(root, _)| root.as_path());
        let result = run_planned_tests(
            &sandbox,
            &cells[0].tests,
            test_root,
            TestSandbox {
                workspace: &build,
                stage: &stage,
                host_arch: rpmx_core::arch::Arch::current()?,
                target_arch: rpmx_core::arch::Arch::parse(arch)?,
                runner,
                source_date_epoch: extracted.source_date_epoch,
                jobs,
                timeout: Some(step_timeout).filter(|seconds| *seconds > 0),
                debuginfo,
            },
        );
        if let Some((root, test_lock)) = prepared {
            let _ = rpmx_core::root::remove_tree(&root);
            let path = output.join(format!(
                "{}-{}-{}.test-root.lock.json",
                cells[0].package, cells[0].version, cells[0].release
            ));
            std::fs::write(&path, test_lock).map_err(|error| {
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

/// Fail before the first step if a declared build tool is neither a package in
/// the locked root nor an executable inside it. Version constraints are parsed
/// off the name and are not checked yet.
fn check_build_tools(
    sandbox: &Sandbox<'_>,
    workspace: &Path,
    tools: &[String],
    host: &[String],
    target: &[String],
    test_tools: &[String],
    locked_packages: &BTreeSet<String>,
) -> Result<(), Error> {
    let mut missing_packages = BTreeSet::new();
    for dependency in host.iter().chain(target).chain(test_tools) {
        let name = dependency.split_whitespace().next().unwrap_or(dependency);
        if !name.is_empty() && !locked_packages.contains(name) {
            missing_packages.insert(name.to_string());
        }
    }
    if !missing_packages.is_empty() && !locked_packages.is_empty() {
        return Err(Error::new(
            codes::ROOT_018,
            format!(
                "build host/test dependencies missing from the selected root: {}; \
                 add them to the profile's packages.install",
                missing_packages.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }

    let mut candidates = BTreeSet::new();
    for tool in tools {
        let name = tool.split_whitespace().next().unwrap_or(tool);
        if name.is_empty() || locked_packages.contains(name) {
            continue;
        }
        candidates.insert(name.to_string());
    }
    if candidates.is_empty() {
        return Ok(());
    }

    let marker = workspace.join(".rpmx-missing-tools");
    let _ = std::fs::remove_file(&marker);
    let mut args = vec![
        "-c".to_string(),
        "for tool do command -v \"$tool\" >/dev/null 2>&1 || printf '%s\n' \"$tool\"; done > /tmp/build/.rpmx-missing-tools".to_string(),
        "sh".to_string(),
    ];
    args.extend(candidates.iter().cloned());
    let step = PlannedStep {
        uses: "tool-probe".to_string(),
        command: Some("sh".to_string()),
        args,
        cwd: None,
        env: BTreeMap::new(),
        needs: Vec::new(),
        tool_digest: None,
        step_digest: None,
        builtin: false,
        inputs: BTreeMap::new(),
        input: None,
        input_file: None,
    };
    sandbox.run(&step).map_err(|error| {
        Error::new(
            codes::ROOT_018,
            format!("cannot probe the build root for tools: {error}"),
        )
    })?;

    let report = std::fs::read_to_string(&marker).unwrap_or_default();
    let _ = std::fs::remove_file(&marker);
    let missing = report.lines().map(str::to_string).collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Error::new(
            codes::ROOT_018,
            format!(
                "build tools missing from the selected root: {}; add them to the profile's packages.install",
                missing.join(", ")
            ),
        ))
    }
}

struct TestSandbox<'a> {
    workspace: &'a Path,
    stage: &'a Path,
    host_arch: rpmx_core::arch::Arch,
    target_arch: rpmx_core::arch::Arch,
    runner: &'a str,
    source_date_epoch: u32,
    jobs: u32,
    timeout: Option<u64>,
    debuginfo: bool,
}

fn run_planned_tests(
    sandbox: &Sandbox<'_>,
    tests: &[rpmx_core::plan::PlannedTest],
    test_root: Option<&Path>,
    settings: TestSandbox<'_>,
) -> Result<(), Error> {
    let build_tests: Vec<_> = tests.iter().filter(|test| test.stage == "build").collect();
    let installed_tests: Vec<_> = tests
        .iter()
        .filter(|test| test.stage == "installed")
        .collect();
    if let Some(test) = tests
        .iter()
        .find(|test| test.stage != "build" && test.stage != "installed")
    {
        return Err(Error::new(
            codes::BUILD_007,
            format!(
                "test '{}' uses unsupported stage '{}'",
                test.name, test.stage
            ),
        ));
    }

    for test in build_tests {
        run_one_test(sandbox, test)?;
    }
    if installed_tests.is_empty() {
        return Ok(());
    }
    let root = test_root.ok_or_else(|| {
        Error::new(
            codes::BUILD_007,
            "installed-stage tests need a test root, which requires a root lock",
        )
    })?;
    let test_backend = match settings.runner {
        "qemu" => SandboxBackend::Qemu(root),
        "bwrap" if settings.host_arch == settings.target_arch => SandboxBackend::Root(root),
        "bwrap" => {
            return Err(Error::new(
                codes::RUNNER_002,
                "bubblewrap cannot run foreign installed tests; use --runner qemu",
            ));
        }
        _ if settings.host_arch == settings.target_arch => SandboxBackend::Root(root),
        _ => SandboxBackend::Qemu(root),
    };
    let test_sandbox = Sandbox {
        backend: test_backend,
        workspace: settings.workspace,
        stage: settings.stage,
        host_arch: settings.host_arch,
        target_arch: settings.target_arch,
        source_date_epoch: settings.source_date_epoch,
        jobs: settings.jobs,
        timeout: settings.timeout,
        debuginfo: settings.debuginfo,
        session: std::cell::RefCell::new(None),
    };
    for test in installed_tests {
        run_one_test(&test_sandbox, test)?;
    }
    Ok(())
}

fn run_one_test(sandbox: &Sandbox<'_>, test: &rpmx_core::plan::PlannedTest) -> Result<(), Error> {
    if let Some(argv) = &test.argv {
        let command = argv.first().ok_or_else(|| {
            Error::new(
                codes::BUILD_007,
                format!("test '{}' has an empty argv", test.name),
            )
        })?;
        let step = PlannedStep {
            uses: format!("test:{}", test.name),
            command: Some(command.clone()),
            args: argv[1..].to_vec(),
            cwd: None,
            env: BTreeMap::new(),
            needs: Vec::new(),
            tool_digest: None,
            step_digest: None,
            builtin: false,
            inputs: BTreeMap::new(),
            input: None,
            input_file: None,
        };
        sandbox.run(&step)?;
    }
    for step in &test.pipeline {
        sandbox.run(step)?;
    }
    Ok(())
}

fn prepare_test_root(
    lock: &rpmx_core::root::RootLock,
    pinned: Option<&Path>,
    test_tools: &[String],
    rpms: &[PathBuf],
    builder_image: Option<&str>,
    work: &Path,
    runner: &str,
) -> Result<(PathBuf, String), Error> {
    let target = rpmx_core::arch::Arch::parse(
        lock.archs
            .first()
            .ok_or_else(|| Error::new(codes::BUILD_007, "the test root has no architecture"))?,
    )?;
    let host = rpmx_core::arch::Arch::current()?;
    let foreign = host != target || runner == "qemu";
    if host != target && !rpmx_core::sandbox::emulation_available(target) {
        return Err(Error::new(
            codes::RUNNER_002,
            format!(
                "cannot prepare a {target} test root on a {host} host; \
                 no qemu-user binfmt handler is available"
            ),
        ));
    }
    let image =
        builder_image.or_else(|| lock.builder.as_ref().map(|builder| builder.image.as_str()));
    let image = image.ok_or_else(|| {
        Error::new(
            codes::BUILD_007,
            "installed-stage tests need a builder image in the root lock",
        )
    })?;

    let mut install = vec!["bash".to_string(), "coreutils".to_string()];
    if foreign {
        // A QEMU-backed test root must boot itself, so it needs a kernel,
        // initramfs, 9p modules and the mount helpers used by the guest init.
        install.extend(
            [
                "kernel-core",
                "kernel-modules-core",
                "kernel-modules",
                "dracut",
                "kmod",
                "util-linux",
                "systemd",
                "systemd-networkd",
                "openssh-server",
                "tar",
            ]
            .iter()
            .map(|package| package.to_string()),
        );
    }
    for tool in test_tools {
        let name = tool.split_whitespace().next().unwrap_or(tool);
        if !name.is_empty() && !install.iter().any(|package| package == name) {
            install.push(name.to_string());
        }
    }

    let profile = rpmx_core::profile::Profile {
        document: rpmx_core::profile::DOCUMENT.to_string(),
        version: rpmx_core::profile::VERSION,
        name: Some("test-root".to_string()),
        releasever: lock.releasever.clone(),
        dist_tag: lock.dist_tag.clone(),
        archs: lock.archs.clone(),
        builder: rpmx_core::profile::Builder {
            image: Some(image.to_string()),
        },
        repositories: lock.repositories.clone(),
        packages: rpmx_core::profile::Packages {
            install: install.clone(),
            reinstall: Vec::new(),
        },
        modules: Default::default(),
        options: lock.options.clone(),
        debuginfo: false,
        sbom: false,
    };

    let cache = crate::commands::fetch::default_cache()?;
    let resolve_work = work.join("test-resolve");
    let assemble_work = work.join("test-assemble");
    let root = work.join("test-root");
    let test_lock = match pinned {
        Some(path) => {
            let pinned = load_lock(path)?;
            check_test_lock(path, &pinned, &install, &lock.archs)?;
            pinned
        }
        None => rpmx_core::root::resolve_profile(&profile, "test-root", &resolve_work, &cache)?,
    };
    let test_lock_text = serde_yaml_ng::to_string(&test_lock)
        .map_err(|error| Error::new(codes::BUILD_007, error.to_string()))?;
    rpmx_core::root::assemble(
        &test_lock,
        rpmx_core::root::AssembleOptions {
            cache: &cache,
            work: &assemble_work,
            root: &root,
        },
    )?;

    let rpms_dir = rpms
        .first()
        .and_then(|path| path.parent())
        .ok_or_else(|| Error::new(codes::BUILD_007, "no binary RPMs were written"))?;
    let mut command = std::process::Command::new("podman");
    command.args([
        "run",
        "--rm",
        "--network=none",
        "--cap-drop=all",
        "--cap-add=CHOWN,DAC_OVERRIDE,FOWNER,FSETID,SETGID,SETUID,SYS_CHROOT",
        "--security-opt=no-new-privileges",
    ]);
    command.arg("--arch").arg(target.to_string());
    let output = command
        .arg("-v")
        .arg(format!("{}:/root:Z", root.display()))
        .arg("-v")
        .arg(format!("{}:/rpms:ro,Z", rpms_dir.display()))
        .arg(image)
        .args([
            "sh",
            "-c",
            "rpm --root /root -i --replacefiles --replacepkgs --oldpackage --noscripts /rpms/*.rpm",
        ])
        .output()
        .map_err(|error| {
            Error::new(
                codes::BUILD_007,
                format!("cannot install test packages: {error}"),
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(
            codes::BUILD_007,
            format!(
                "cannot install the built RPMs into the test root: {}; \
                 add missing runtime packages to build.dependencies.tests \
                 or pass a pinned --test-root-lock",
                stderr.trim()
            ),
        ));
    }
    Ok((root, test_lock_text))
}

/// A pinned test root must contain the packages the recipe asks tests to install,
/// otherwise the lock and the recipe have drifted apart.
fn check_test_lock(
    path: &Path,
    lock: &rpmx_core::root::RootLock,
    required: &[String],
    archs: &[String],
) -> Result<(), Error> {
    let arch = archs
        .first()
        .ok_or_else(|| Error::new(codes::BUILD_007, "the test root lock has no architectures"))?;
    let installed = lock.packages.get(arch).ok_or_else(|| {
        Error::new(
            codes::BUILD_007,
            format!("the test root lock has no packages for {arch}"),
        )
    })?;
    let names: BTreeSet<&str> = installed
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let missing: Vec<&str> = required
        .iter()
        .map(|dependency| dependency.split_whitespace().next().unwrap_or(dependency))
        .filter(|name| !name.is_empty() && !names.contains(name))
        .collect();
    if !missing.is_empty() {
        return Err(Error::new(
            codes::BUILD_007,
            format!(
                "test root lock {} is missing test dependencies: {}",
                path.display(),
                missing.join(", ")
            ),
        ));
    }
    Ok(())
}
