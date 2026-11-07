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
    /// Where steps execute: the host for a cross build, the target otherwise.
    pub exec: rpmx_core::arch::Arch,
    /// Target sysroot, mounted read-only, for a cross build.
    pub sysroot: Option<&'a Path>,
    /// Toolchain commands for a cross build.
    pub cross: Option<&'a rpmx_core::guest::CrossToolchain>,
    /// Package names in the target sysroot. Host dependencies are satisfied
    /// by either root.
    pub sysroot_packages: Option<&'a std::collections::BTreeSet<String>>,
    pub runner: &'a str,
    pub cell: Option<&'a str>,
    pub policy: PlanPolicy<'a>,
    pub jobs: NonZeroU32,
    pub step_timeout: NonZeroU64,
    pub root_image: Option<&'a str>,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub runner_asset_root: Option<&'a Path>,
    pub cache: Option<&'a Path>,
    pub offline: bool,
    /// Re-hash an explicit root against its lock. A full build that just
    /// resolved or cached the root passes false.
    pub verify_root: bool,
    pub run_tests: bool,
    /// Skip installed-stage tests, which execute target binaries.
    pub skip_installed: bool,
    /// Keep the prepared test root here instead of deleting it.
    pub save_test_root: Option<&'a Path>,
    pub test_filter: &'a [String],
    pub no_index: bool,
    pub no_lint: bool,
    pub junit: Option<&'a Path>,
    pub sbom: Option<&'a Path>,
    pub test_root_lock: Option<&'a Path>,
    pub signing: &'a OptionalSigningArgs,
    pub quiet: bool,
    /// Skip the embedded-lock comparison and record the old roots in the new
    /// SRPM. Only the bootstrap rebuild path sets this.
    pub replace_root_lock: bool,
    /// Write a replacement SRPM here after the binary rebuild.
    pub srpm_out: Option<&'a Path>,
}

pub struct RebuildOutput {
    pub rpms: Vec<PathBuf>,
    pub srpms: Vec<PathBuf>,
    pub evidence: Vec<PathBuf>,
}

pub fn run(args: RebuildArgs) -> Result<Vec<PathBuf>, Error> {
    run_with(args, false, None).map(|output| output.rpms)
}

/// Run a rebuild with the bootstrap replacement semantics.
pub fn run_with(
    args: RebuildArgs,
    replace_root_lock: bool,
    srpm_out: Option<&Path>,
) -> Result<RebuildOutput, Error> {
    if let Some(path) = args.junit.as_deref() {
        let _ = std::fs::remove_file(path);
    }
    let resolved = crate::commands::resolve_policy(crate::commands::PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    // A cross SRPM records its target in the header; rebuild it for that
    // architecture without asking the caller to repeat it.
    let arch = match &args.arch {
        Some(arch) => arch.clone(),
        None => crate::commands::srpm_arch(&args.file)?,
    };
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| crate::commands::default_output(&arch));
    execute(RebuildRequest {
        file: &args.file,
        output: &output,
        arch: &arch,
        exec: rpmx_core::arch::Arch::parse(&arch)?,
        sysroot: None,
        cross: None,
        sysroot_packages: None,
        runner: &args.runner,
        cell: args.cell.as_deref(),
        policy: resolved.plan_policy(),
        jobs: args.runtime.jobs,
        step_timeout: args.runtime.step_timeout,
        root_image: args.root_image.as_deref(),
        root_lock: args.root_lock.as_deref(),
        root: args.root.as_deref(),
        runner_asset_root: args.runner_asset_root.as_deref(),
        cache: args.runtime.cache.cache.as_deref(),
        offline: args.offline,
        verify_root: true,
        run_tests: !args.no_tests,
        skip_installed: args.skip_installed,
        save_test_root: args.save_test_root.as_deref(),
        test_filter: &args.test,
        no_index: args.no_index,
        no_lint: args.no_lint,
        junit: args.junit.as_deref(),
        sbom: args.sbom.as_deref(),
        test_root_lock: args.test_root_lock.as_deref(),
        signing: &args.signing,
        quiet: args.quiet,
        replace_root_lock,
        srpm_out,
    })
}

pub fn execute(request: RebuildRequest<'_>) -> Result<RebuildOutput, Error> {
    let build_started = std::time::Instant::now();
    let signing = crate::commands::load_signing_key(
        request.signing.signing_key.as_deref(),
        request.signing.signing_passphrase_env.as_deref(),
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
        builder_image,
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
    let run_tests = request.run_tests;
    let sbom_path = request.sbom;
    let quiet = request.quiet;
    let json = rpmx_core::output::json();
    let jobs = request.jobs;
    let step_timeout = request.step_timeout;
    let runner = request.runner;
    let arch = target.as_str();

    let host_arch = rpmx_core::arch::Arch::current()?;
    let target_arch = rpmx_core::arch::Arch::parse(arch)?;
    let runner_kind = runner.parse::<RunnerKind>()?;
    let sbom_enabled = active_lock.as_ref().map(|lock| lock.sbom).unwrap_or(true);
    let prefix = active_lock
        .as_ref()
        .map(|lock| lock.prefix.as_str())
        .unwrap_or(rpmx_core::guest::PREFIX);
    let libdir = active_lock
        .as_ref()
        .map(|lock| lock.libdir.as_str())
        .unwrap_or(rpmx_core::guest::LIBDIR);
    // An explicit flag wins over the SRPM; otherwise the embedded cross block
    // supplies the sysroot and toolchain.
    let effective_sysroot: Option<&std::path::Path> = request.sysroot.or(sysroot.as_deref());
    let effective_cross = request.cross.or(cross.as_ref());
    // A foreign execution without a cross profile is emulation. Say so once
    // per build: the difference is roughly 15x, and it is a choice.
    if exec != host_arch && effective_cross.is_none() {
        rpmx_core::output::warning(
            "emulation",
            format!(
                "executing {arch} steps through qemu-user, roughly 15x slower; \
                 a profile with a cross block builds them natively \
                 (see docs/usage/cross-compilation.md)"
            ),
        );
    }
    let sandbox = Sandbox::new(SandboxConfig {
        runner: runner_kind,
        host: host_arch,
        exec,
        target: target_arch,
        sysroot: effective_sysroot,
        cross: effective_cross,
        workspace: &build,
        stage: &stage,
        root: assembled_root.as_deref(),
        assets: rpmx_core::runner::RunnerAssets::new(assembled_appliance.as_deref(), &[]),
        image: request.root_image,
        prefix,
        libdir,
        source_date_epoch: extracted.source_date_epoch,
        jobs,
        timeout: step_timeout,
    })?;

    // The source snapshot carries declaration directories for every matrix
    // cell. The SRPM inventory already includes their source-time components;
    // only the selected cell's build-time declarations apply to this rebuild.
    let declaration_root = build.join(rpmx_core::components::BUNDLED_DIR);
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
    rpmx_core::components::prepare_output_dirs(&build, &[&cell])?;

    {
        let _phase = rpmx_core::output::enter_phase("build");
        // A host dependency is satisfied by either root: the build root holds
        // native packages, the sysroot holds target ones.
        let mut available = locked_packages.clone();
        // An automatic root was resolved from a profile that already included
        // these dependencies, and resolution may have substituted a virtual
        // provide (`zlib-devel` -> `zlib-ng-compat-devel`) that the lock
        // records under the provider's name. Only an explicit root needs the
        // names checked literally.
        let trusted = !request.verify_root;
        if effective_sysroot.is_some() {
            if let Some(packages) = request.sysroot_packages {
                available.extend(packages.iter().cloned());
            }
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
            available.extend(cell.host.iter().chain(cell.target.iter()).cloned());
        } else if trusted {
            available.extend(cell.host.iter().chain(cell.target.iter()).cloned());
        }
        // A cross build installs test dependencies into a target test root
        // when the tests run, not into the build root or the sysroot.
        let test_tools: &[String] = if effective_sysroot.is_some() {
            &[]
        } else {
            &cell.test_tools
        };
        check_build_tools(
            &sandbox,
            &build,
            &cell.tools,
            &cell.host,
            &cell.target,
            test_tools,
            &available,
        )?;
        rpmx_core::output::phase("build", format!("{arch} · {} steps", cell.pipeline.len()));
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
            sandbox.run_numbered(step, index + 1, total_steps)?;
        }

        std::fs::create_dir_all(&output).map_err(|error| {
            Error::new(
                codes::BUILD_001,
                format!("cannot create {}: {error}", output.display()),
            )
        })?;

        sandbox.sync()?;
    }

    let build_inventory = rpmx_core::components::discover(&build, &[&cell])?;
    bundled.merge(build_inventory)?;
    bundled.warn_declared();

    // The link-closure check runs against whatever root the package was
    // built and linked in: the sysroot for cross, the build root otherwise.
    // A host root is not a reliable index, so it is skipped.
    let link_root = effective_sysroot.or(assembled_root.as_deref());
    if let Some(sysroot) = link_root {
        let missing = rpmx_core::elf::unresolved_needed(&stage, sysroot).map_err(|error| {
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
        rpmx_core::output::phase(
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
    rpmx_core::output::phase(
        "package",
        format!(
            "{arch} · {} · {}",
            packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            rpmx_core::output::duration(package_started)
        ),
    );

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
        rpmx_core::output::wrote(&path);
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
            sysroot_lock: sysroot_lock.as_ref(),
            builder_image: builder_image.as_deref(),
            pinned_lock: request.test_root_lock,
            arch,
            runner: runner_kind,
            appliance: assembled_appliance.as_deref(),
            source_date_epoch: extracted.source_date_epoch,
            jobs,
            timeout: step_timeout,
            skip_installed: request.skip_installed,
            save_test_root: request.save_test_root,
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
        rpmx_core::output::phase(
            "index",
            format!(
                "{} in {}",
                rpmx_core::output::count(report.packages, "package", "packages"),
                output.display()
            ),
        );
        repository.push(report.repomd);
        if let Some(signature) = report.signature {
            repository.push(signature);
        }
    }
    let srpms = if let Some(srpm_out) = request.srpm_out {
        write_replacement_srpm(
            srpm_out,
            request.file,
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

    report::print_artifacts(
        quiet,
        json,
        &written,
        &srpms,
        &evidence,
        &repository,
        signing.as_ref().map(|key| key.fingerprint()),
    )?;
    if !quiet {
        rpmx_core::output::phase(
            "done",
            format!(
                "{} in {} · {}",
                rpmx_core::output::count(
                    written.len() + srpms.len() + evidence.len() + repository.len(),
                    "artifact",
                    "artifacts"
                ),
                rpmx_core::output::display_path(&output),
                rpmx_core::output::duration(build_started)
            ),
        );
    }
    Ok(RebuildOutput {
        rpms: written,
        srpms,
        evidence,
    })
}

#[allow(clippy::too_many_arguments)]
fn write_replacement_srpm(
    output: &Path,
    input: &Path,
    work: &Path,
    bundle: &rpmx_core::RecipeBundle,
    plan: &rpmx_core::Plan,
    pipelines: &Path,
    bundled: &rpmx_core::components::Inventory,
    root_lock: Option<&rpmx_core::root::RootLock>,
    sysroot_lock: Option<&rpmx_core::root::RootLock>,
    cross: Option<&rpmx_core::guest::CrossToolchain>,
    replaced: &Option<rpmx_core::srpm::ReplacedLock>,
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
        rpmx_core::source::unpack_snapshot(&snapshot, &sources)?;
    }
    // `prepare_source_packages` reads the inventory from the source tree and
    // then excludes it from the source snapshot, exactly as the original SRPM
    // writer did.
    std::fs::write(
        sources.join(rpmx_core::components::INVENTORY_FILE),
        bundled.to_json()?,
    )
    .map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot write the bundled inventory: {error}"),
        )
    })?;
    let recipe_bytes = bundle.root_file().bytes.clone();
    let includes: Vec<rpmx_core::source_package::RecipeInclude> = bundle
        .included_files()
        .map(|file| rpmx_core::source_package::RecipeInclude {
            name: file.name.clone(),
            bytes: file.bytes.clone(),
        })
        .collect();
    let packages = rpmx_core::source_package::prepare_source_packages(
        rpmx_core::source_package::SourcePackageOptions {
            recipe: &bundle.recipe,
            plan,
            recipe_bytes: &recipe_bytes,
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
        let path = crate::commands::emit::source(&package, output, signing, timestamp)?;
        rpmx_core::output::wrote(&path);
        written.push(path);
    }
    Ok(written)
}
