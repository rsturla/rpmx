use std::path::{Path, PathBuf};

use crate::domain::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use crate::domain::build_env::check_build_tools;
use crate::domain::error::codes;
use crate::domain::runner::RunnerKind;
use crate::domain::sandbox::{Sandbox, SandboxConfig};
use crate::domain::Error;
use crate::emit;

#[path = "prepare.rs"]
mod prepare;
#[path = "sbom.rs"]
mod sbom;

use prepare::{prepare, RebuildState};
use sbom::SbomContext;

struct RebuildContext<'a> {
    config: &'a crate::ApplicationConfig,
    input: &'a crate::RebuildInput,
    output: &'a Path,
    arch: &'a str,
    policy: crate::policy::PlanPolicy<'a>,
    prepared: Option<&'a crate::environment::Environment>,
}

pub(crate) fn execute(
    config: &crate::ApplicationConfig,
    input: &crate::RebuildInput,
    output: &Path,
    run_report: &mut crate::RunReport,
) -> Result<(), Error> {
    let arch = input
        .arch
        .clone()
        .map(Ok)
        .unwrap_or_else(|| rpmx_rpm::package_arch(&input.srpm))?;
    let policy = crate::environment::policy(config)?;
    rebuild(
        RebuildContext {
            config,
            input,
            output,
            arch: &arch,
            policy: policy.plan_policy(),
            prepared: None,
        },
        run_report,
    )
}

pub(crate) fn execute_prepared(
    config: &crate::ApplicationConfig,
    input: &crate::RebuildInput,
    output: &Path,
    prepared: &crate::environment::Environment,
    report: &mut crate::RunReport,
) -> Result<(), Error> {
    let policy = crate::environment::policy(config)?;
    rebuild(
        RebuildContext {
            config,
            input,
            output,
            arch: prepared.target.as_str(),
            policy: policy.plan_policy(),
            prepared: Some(prepared),
        },
        report,
    )
}

fn rebuild(request: RebuildContext<'_>, run_report: &mut crate::RunReport) -> Result<(), Error> {
    let build_started = std::time::Instant::now();
    run_report.build_tests_requested = request.input.build_tests;
    let signing = crate::support::load_signing_key(
        request.config.signing.signing_key.as_deref(),
        request.config.signing.signing_passphrase_env.as_deref(),
    )?;
    let state = prepare(&request, signing.as_ref())?;
    let RebuildState {
        work,
        extracted,
        package_lock,
        mut bundled,
        recipe,
        cell,
        build,
        stage,
        active_lock,
        locked_packages,
        assembled_root,
        assembled_appliance,
        signature_timestamp,
        exec,
        sysroot,
        cross,
        sysroot_lock,
        target,
        bundle,
        plan,
        pipelines,
        replaced,
    } = state;
    let output = request.output.to_path_buf();
    let sbom_path = request.input.sbom.as_deref();
    let quiet = request.config.quiet;
    let jobs = request.config.jobs;
    let step_timeout = request.config.timeout;
    let runner = request.config.runner.as_str();
    let arch = target.as_str();

    let host_arch = crate::domain::arch::Arch::current()?;
    let target_arch = crate::domain::arch::Arch::parse(arch)?;
    let runner_kind = runner.parse::<RunnerKind>()?;
    let sbom_enabled = active_lock.as_ref().map(|lock| lock.sbom).unwrap_or(true);
    let prefix = active_lock
        .as_ref()
        .map(|lock| lock.prefix.as_str())
        .unwrap_or(crate::domain::guest::PREFIX);
    let libdir = active_lock
        .as_ref()
        .map(|lock| lock.libdir.as_str())
        .unwrap_or(crate::domain::guest::LIBDIR);
    // An explicit flag wins over the SRPM; otherwise the embedded cross block
    // supplies the sysroot and toolchain.
    let effective_sysroot = request
        .prepared
        .and_then(|env| env.sysroot.as_ref().map(|root| root.path()))
        .or(sysroot.as_deref());
    let effective_cross = request
        .prepared
        .and_then(|env| env.cross.as_ref())
        .or(cross.as_ref());
    let selected_root = request
        .prepared
        .and_then(|env| env.root())
        .or_else(|| assembled_root.as_ref().map(|root| root.path()));
    // A foreign execution without a cross profile is emulation. Say so once
    // per build: the difference is roughly 15x, and it is a choice.
    if exec != host_arch && effective_cross.is_none() {
        crate::domain::output::warning(
            "emulation",
            format!(
                "executing {arch} steps through qemu-user, roughly 15x slower; \
                 a profile with a cross block builds them natively \
                 (see docs/usage/cross-compilation.md)"
            ),
        );
    }
    let sandbox_config = SandboxConfig {
        writable_root: false,
        runner: runner_kind,
        host: host_arch,
        exec,
        target: target_arch,
        sysroot: effective_sysroot,
        cross: effective_cross,
        workspace: &build,
        stage: &stage,
        root: selected_root,
        assets: crate::domain::runner::RunnerAssets::new(assembled_appliance.as_deref(), &[]),
        image: None,
        prefix,
        libdir,
        source_date_epoch: extracted.source_date_epoch,
        jobs,
        timeout: step_timeout,
    };
    let sandbox = Sandbox::new(sandbox_config)?;

    // The source snapshot carries declaration directories for every matrix
    // cell. The SRPM inventory already includes their source-time components;
    // only the selected cell's build-time declarations apply to this rebuild.
    let declaration_root = build.join(crate::domain::components::BUNDLED_DIR);
    match std::fs::remove_dir_all(&declaration_root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(Error::new(
                codes::BUNDLED_003,
                format!("cannot clear {}: {error}", declaration_root.display()),
            ))
        }
    }
    crate::domain::components::prepare_output_dirs(&build, &[&cell])?;

    let prepared_tests = if request.input.build_tests {
        let prepared = super::tests::prepare(
            request.config,
            &cell,
            active_lock.as_ref(),
            sysroot_lock.as_ref(),
            sandbox_config,
            &output,
            run_report,
        );
        prepared.require_ready(run_report)?;
        Some(prepared)
    } else {
        None
    };
    let pipeline_evidence = run_report
        .evidence_directory(&output)
        .join("build")
        .join(arch)
        .join(&cell.slug);
    std::fs::create_dir_all(&pipeline_evidence)
        .map_err(|error| Error::new(codes::BUILD_006, error.to_string()))?;
    let logs = crate::domain::runner::OutputLogs {
        stdout: std::fs::File::create(pipeline_evidence.join("stdout.log"))
            .map_err(|error| Error::new(codes::BUILD_006, error.to_string()))?,
        stderr: std::fs::File::create(pipeline_evidence.join("stderr.log"))
            .map_err(|error| Error::new(codes::BUILD_006, error.to_string()))?,
    };
    run_report.evidence.push(pipeline_evidence);
    let pipeline = (|| -> Result<(), Error> {
        let _phase = crate::domain::output::enter_phase("build");
        // A host dependency is satisfied by either root: the build root holds
        // native packages, the sysroot holds target ones.
        let mut available = locked_packages.clone();
        // An automatic root was resolved from a profile that already included
        // these dependencies, and resolution may have substituted a virtual
        // provide (`zlib-devel` -> `zlib-ng-compat-devel`) that the lock
        // records under the provider's name. Only an explicit root needs the
        // names checked literally.
        let trusted = request.prepared.is_some();
        include_resolved_build_requirements(&mut available, &cell.packages, trusted);
        if effective_sysroot.is_some() {
            if let Some(lock) = &sysroot_lock {
                available.extend(
                    lock.packages
                        .values()
                        .flatten()
                        .map(|package| package.name.clone()),
                );
            }
            // The sysroot lock records resolved package names, not virtual
            // provides, so a name such as `zlib-devel` may resolve to
            // `zlib-ng-compat-devel`. Resolution already proved the declared
            // dependency exists; the link-closure check proves the library
            // does, which is the stronger claim.
            available.extend(cell.sysroot.iter().cloned());
        } else if trusted {
            available.extend(cell.sysroot.iter().cloned());
        }
        check_build_tools(&sandbox, &build, &cell.packages, &cell.sysroot, &available)?;
        crate::domain::output::phase("build", format!("{arch} · {} steps", cell.pipeline.len()));
        let total_steps = cell.pipeline.len();
        for (index, step) in cell.pipeline.iter().enumerate() {
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
            sandbox.run_recorded(step, index + 1, total_steps, &logs)?;
        }

        std::fs::create_dir_all(&output).map_err(|error| {
            Error::new(
                codes::BUILD_001,
                format!("cannot create {}: {error}", output.display()),
            )
        })?;

        Ok(())
    })();
    let finished = sandbox.finish();
    if let Err(error) = pipeline {
        if let Err(secondary) = finished {
            run_report
                .secondary
                .push(crate::Diagnostic::from(&secondary));
        }
        return Err(error);
    }
    finished?;

    // The producer is stopped before making an independent payload snapshot.
    // Checks never get a writable alias to the tree used for packaging.
    let sealed_stage = work.path().join("sealed-stage");
    rpmx_runner::test_runner::copy_workspace(&stage, &sealed_stage)?;
    let stage = sealed_stage;

    let build_inventory = crate::domain::components::discover(&build, &[&cell])?;
    bundled.merge(build_inventory)?;
    bundled.warn_declared();

    // The link-closure check runs against whatever root the package was
    // built and linked in: the sysroot for cross, the build root otherwise.
    // A host root is not a reliable index, so it is skipped.
    let link_root = effective_sysroot.or(selected_root);
    if let Some(sysroot) = link_root {
        let missing = crate::domain::elf::unresolved_needed(&stage, sysroot).map_err(|error| {
            Error::new(
                codes::BUILD_009,
                format!("cannot inspect the staged payload: {error}"),
            )
        })?;
        if !missing.is_empty() {
            let detail = missing
                .iter()
                .take(10)
                .map(|(file, needed)| format!("{file}: {needed}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::new(
                codes::BUILD_009,
                format!(
                    "the staged payload needs libraries that are not in the {arch} root: {detail}"
                ),
            ));
        }
        crate::domain::output::phase(
            "link",
            format!("{arch} · every DT_NEEDED resolves inside the root"),
        );
    }

    let package_started = std::time::Instant::now();
    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &cell,
        stage: &stage,
        arch,
        source_date_epoch: extracted.source_date_epoch,
        bundled: &bundled,
    })?;
    crate::domain::output::phase(
        "package",
        format!(
            "{arch} · {} · {}",
            packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            crate::domain::output::duration(package_started)
        ),
    );

    if request.input.lint {
        let findings = crate::package_lint::check(&packages);
        crate::package_lint::enforce(&findings)?;
    }

    let sbom_policy = sbom_enabled || sbom_path.is_some();
    let sbom = if sbom_policy {
        Some(SbomContext::prepare(&package_lock, work.path(), &cell))
    } else {
        None
    };

    let private_output = work.path().join("private-candidates");
    std::fs::create_dir_all(&private_output)
        .map_err(|error| Error::new(codes::BUILD_001, error.to_string()))?;
    let mut written = Vec::new();
    let mut evidence = Vec::new();
    let mut built = Vec::new();
    for mut package in packages {
        if let Some(sbom) = &sbom {
            sbom.attach_package(&mut package, &recipe)?;
        }
        let path = emit::binary(
            &package,
            &private_output,
            signing.as_ref(),
            signature_timestamp,
        )?;
        crate::domain::output::wrote(&path);
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
                |(path, name, version, release, arch)| crate::domain::sbom::BinaryRef {
                    name,
                    version,
                    release,
                    arch,
                    license: &recipe.package.license,
                    path,
                },
            )
            .collect::<Vec<_>>();
        let final_path = sbom_path
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| {
                output.join(format!(
                    "{}-{}-{}.build.spdx.json",
                    cell.package, cell.version, cell.release
                ))
            });
        let path = private_output.join(final_path.file_name().expect("SBOM filename"));
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
        evidence.push((path, final_path));
    }

    if let Some(prepared) = prepared_tests {
        super::tests::execute(
            prepared,
            &bundle,
            sandbox_config,
            &stage,
            &written,
            &output,
            run_report,
        )?;
    }
    // Publication only copies bytes already checked. Packaging, dependency
    // discovery and component scanning are never repeated after tests.
    for path in &written {
        let destination = output.join(path.file_name().expect("RPM filename"));
        publish_file(path, &destination)?;
        run_report.artifacts.rpms.push(destination);
    }
    for (path, destination) in evidence {
        publish_file(&path, &destination)?;
        run_report.evidence.push(destination);
    }

    let mut repository = Vec::new();
    if request.input.index {
        let report = index_repository(
            &output,
            signing.as_ref(),
            signature_timestamp.unwrap_or(extracted.source_date_epoch),
        )?;
        crate::domain::output::phase(
            "index",
            format!(
                "{} in {}",
                crate::domain::output::count(report.packages, "package", "packages"),
                output.display()
            ),
        );
        repository.push(report.repomd);
        if let Some(signature) = report.signature {
            repository.push(signature);
        }
    }
    let srpms = if let Some(srpm_out) = request.input.replacement_srpm.as_deref() {
        write_replacement_srpm(
            srpm_out,
            request.input.srpm.as_path(),
            work.path(),
            &bundle,
            &plan,
            &pipelines,
            &bundled,
            active_lock.as_ref(),
            sysroot_lock.as_ref(),
            effective_cross,
            &replaced,
            sbom_enabled,
            extracted.source_date_epoch,
            signing.as_ref(),
            signature_timestamp,
        )?
    } else {
        Vec::new()
    };

    if !quiet {
        crate::domain::output::phase(
            "done",
            format!(
                "{} in {} · {}",
                crate::domain::output::count(
                    written.len() + srpms.len() + repository.len(),
                    "artifact",
                    "artifacts"
                ),
                crate::domain::output::display_path(&output),
                crate::domain::output::duration(build_started)
            ),
        );
    }
    run_report.artifacts.srpms.extend(srpms);
    run_report.artifacts.repository.extend(repository);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_replacement_srpm(
    output: &Path,
    input: &Path,
    work: &Path,
    bundle: &crate::domain::RecipeBundle,
    plan: &crate::domain::Plan,
    pipelines: &Path,
    bundled: &crate::domain::components::Inventory,
    root_lock: Option<&crate::domain::root::RootLock>,
    sysroot_lock: Option<&crate::domain::root::RootLock>,
    cross: Option<&crate::domain::guest::CrossToolchain>,
    replaced: &Option<crate::domain::srpm::ReplacedLock>,
    sbom: bool,
    source_date_epoch: u32,
    signing: Option<&rpmx_rpm::sign::SigningKey>,
    timestamp: Option<u32>,
) -> Result<Vec<PathBuf>, Error> {
    // The build pipeline may have modified its workspace. Re-extract the
    // original snapshot so the replacement SRPM carries pristine sources.
    let sources = work.join("srpm-source");
    let snapshot = work.join("RPMX-SOURCE.tar.gz");
    if sources.exists() {
        std::fs::remove_dir_all(&sources).map_err(|error| {
            Error::new(
                codes::SRPM_002,
                format!("cannot clear {}: {error}", sources.display()),
            )
        })?;
    }
    std::fs::create_dir_all(&sources).map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot create {}: {error}", sources.display()),
        )
    })?;
    if snapshot.exists() {
        crate::domain::source::unpack_snapshot(&snapshot, &sources)?;
    }
    // `prepare_source_packages` reads the inventory from the source tree and
    // then excludes it from the source snapshot, exactly as the original SRPM
    // writer did.
    std::fs::write(
        sources.join(crate::domain::components::INVENTORY_FILE),
        bundled.to_json()?,
    )
    .map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot write the bundled inventory: {error}"),
        )
    })?;
    let recipe_bytes = bundle.root_file().bytes.clone();
    let includes: Vec<crate::domain::source_package::RecipeInclude> = bundle
        .included_files()
        .map(|file| crate::domain::source_package::RecipeInclude {
            name: file.name.clone(),
            bytes: file.bytes.clone(),
            mode: file.mode,
        })
        .collect();
    let packages = crate::domain::source_package::prepare_source_packages(
        crate::domain::source_package::SourcePackageOptions {
            recipe: &bundle.recipe,
            plan,
            recipe_bytes: &recipe_bytes,
            recipe_mode: bundle.root_file().mode,
            recipe_digest: Some(bundle.digest()),
            recipe_includes: &includes,
            step_root: Some(pipelines),
            sources: &sources,
            source_date_epoch,
            root_lock,
            sysroot_lock,
            cross,
            sbom,
            replaced: replaced.as_ref(),
        },
    )?;
    std::fs::create_dir_all(output).map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot create {}: {error}", output.display()),
        )
    })?;
    let input = std::fs::canonicalize(input).ok();
    let mut written = Vec::new();
    for package in packages {
        let path = output.join(format!(
            "{}-{}-{}.src.rpm",
            package.name, package.version, package.release
        ));
        if input.as_deref() == std::fs::canonicalize(&path).ok().as_deref() {
            return Err(Error::new(
                codes::SRPM_002,
                format!(
                    "refusing to overwrite the input SRPM {}; choose a different --srpm-out",
                    input
                        .as_deref()
                        .unwrap_or(input.as_deref().unwrap_or(Path::new("")))
                        .display()
                ),
            ));
        }
        let path = crate::support::emit::source(&package, output, signing, timestamp)?;
        crate::domain::output::wrote(&path);
        written.push(path);
    }
    Ok(written)
}

// Automatic resolution already proved these requirements, including virtual
// capabilities whose names are not executable or installed package names.
fn include_resolved_build_requirements(
    available: &mut std::collections::BTreeSet<String>,
    requirements: &[String],
    automatic: bool,
) {
    if automatic {
        available.extend(
            requirements.iter().map(|requirement| {
                crate::domain::build_env::dependency_name(requirement).to_string()
            }),
        );
    }
}

fn index_repository(
    output: &Path,
    key: Option<&rpmx_rpm::sign::SigningKey>,
    timestamp: u32,
) -> Result<rpmx_rpm::repo::RepoReport, Error> {
    rpmx_rpm::repo::generate(rpmx_rpm::repo::RepoOptions {
        rpms: output,
        output,
        signing_key: key,
        timestamp,
    })
}

fn publish_file(source: &Path, destination: &Path) -> Result<(), Error> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::new(codes::BUILD_006, error.to_string()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| Error::new(codes::BUILD_006, error.to_string()))?;
    let mut input = std::fs::File::open(source)
        .map_err(|error| Error::new(codes::BUILD_006, error.to_string()))?;
    std::io::copy(&mut input, &mut temporary)
        .map_err(|error| Error::new(codes::BUILD_006, error.to_string()))?;
    temporary.persist(destination).map_err(|error| {
        Error::new(
            codes::BUILD_006,
            format!("cannot publish {}: {}", destination.display(), error.error),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod automatic_root_tests {
    use super::include_resolved_build_requirements;
    use std::collections::BTreeSet;

    #[test]
    fn automatic_roots_trust_resolved_virtual_build_capabilities() {
        let requested = vec!["gcc-c++ >= 16".to_string()];
        let mut automatic = BTreeSet::from(["gcc-16-default".to_string()]);
        include_resolved_build_requirements(&mut automatic, &requested, true);
        assert!(automatic.contains("gcc-c++"));

        let mut explicit = BTreeSet::from(["gcc-16-default".to_string()]);
        include_resolved_build_requirements(&mut explicit, &requested, false);
        assert!(!explicit.contains("gcc-c++"));
    }
}
