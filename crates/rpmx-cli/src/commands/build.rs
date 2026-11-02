use std::num::NonZeroU32;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};

use crate::commands::emit;
use rpmx_core::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use rpmx_core::error::codes;
use rpmx_core::Arch;
use rpmx_core::Error;

use crate::commands::{
    load_plan, pipeline::BuildPipeline, resolve_policy, BuildArgs, PlanPolicy, PolicyInput,
};

#[derive(Debug, Default)]
struct ArchArtifacts {
    rpms: Vec<PathBuf>,
    srpms: Vec<PathBuf>,
    evidence: Vec<PathBuf>,
    repository: Vec<PathBuf>,
}

impl ArchArtifacts {
    fn merge(&mut self, other: Self) {
        self.rpms.extend(other.rpms);
        self.srpms.extend(other.srpms);
        self.evidence.extend(other.evidence);
        self.repository.extend(other.repository);
    }

    fn count(&self) -> usize {
        self.rpms.len() + self.srpms.len() + self.evidence.len() + self.repository.len()
    }
}

pub fn run(args: BuildArgs, pipeline_dir: Option<&Path>) -> Result<Vec<PathBuf>, Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    if let Some(path) = args.junit.as_deref() {
        let _ = std::fs::remove_file(path);
    }
    let profile = args.policy.profile.clone();
    let arches = resolve_arches(&args)?;

    if let Some(stage) = args.stage.as_deref() {
        if arches.len() > 1 {
            return Err(Error::new(
                codes::BUILD_003,
                "--stage packages one architecture at a time; pass one --arch",
            ));
        }
        let output = arch_output(&args, &arches[0]);
        return package_stage(&args, stage, &output, pipeline_dir, policy, &arches[0]);
    }

    build_from_recipe(&args, &arches, pipeline_dir, policy, profile.as_deref())
}

/// Expand the CLI architecture list. An empty list means the native arch;
/// `all` means every architecture rpmx knows.
fn resolve_arches(args: &BuildArgs) -> Result<Vec<String>, Error> {
    let mut expanded = Vec::new();
    let mut all = false;
    if args.arch.is_empty() {
        // A profile that declares exactly one target is explicit about what
        // this recipe builds; a cross profile must not silently fall back to
        // the host architecture.
        match profile_arches(args)? {
            Some(allowed) if allowed.len() == 1 => expanded.extend(allowed),
            _ => expanded.push(rpmx_core::arch::native_arch().to_string()),
        }
    } else {
        for value in &args.arch {
            if value == "all" {
                all = true;
                expanded.extend(Arch::ALL.iter().map(|arch| arch.as_str().to_string()));
            } else {
                expanded.push(value.clone());
            }
        }
    }

    if all {
        if let Some(allowed) = profile_arches(args)? {
            expanded.retain(|arch| {
                Arch::parse(arch)
                    .map(|arch| allowed.iter().any(|value| value == arch.as_str()))
                    .unwrap_or(false)
            });
        }
        if args.runner == "qemu" {
            expanded.retain(|arch| matches!(arch.as_str(), "x86_64" | "aarch64"));
        }
    }

    let mut arches: Vec<String> = Vec::new();
    for value in expanded {
        let arch = Arch::parse(&value).map_err(|error| {
            Error::new(
                codes::CLI_001,
                format!("invalid --arch '{value}': {}", error.message),
            )
        })?;
        let name = arch.as_str().to_string();
        if arches.contains(&name) {
            return Err(Error::new(
                codes::CLI_001,
                format!("--arch '{name}' is listed more than once"),
            ));
        }
        arches.push(name);
    }
    if arches.is_empty() {
        return Err(Error::new(
            codes::CLI_001,
            "no architectures are allowed by this profile and runner",
        ));
    }
    Ok(arches)
}

/// The effective profile's allowed architecture set, when a root profile is
/// named. `all` intersects with it so a single-arch profile does not fail the
/// whole command.
fn profile_arches(args: &BuildArgs) -> Result<Option<Vec<String>>, Error> {
    let bundle = rpmx_core::load_recipe_bundle(&args.file)?;
    let path = if let Some(path) = args.policy.profile.as_deref() {
        Some(path.to_path_buf())
    } else {
        bundle
            .recipe
            .build
            .root
            .as_ref()
            .and_then(|root| root.profile.as_ref())
            .map(|profile| {
                bundle
                    .root
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(profile)
            })
    };
    let Some(path) = path else {
        return Ok(None);
    };
    let profile = rpmx_core::profile::load_profile(&path)?;
    if profile.archs.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        profile
            .archs
            .iter()
            .map(|arch| {
                Arch::parse(arch)
                    .map(|arch| arch.as_str().to_string())
                    .map_err(|error| Error::new(codes::CLI_001, error.message))
            })
            .collect::<Result<_, _>>()?,
    ))
}

fn arch_output(args: &BuildArgs, arch: &str) -> PathBuf {
    output_path(args.output.as_deref(), arch)
}

fn output_path(base: Option<&Path>, arch: &str) -> PathBuf {
    base.unwrap_or_else(|| Path::new("packages")).join(arch)
}

fn resolve_arch_jobs(value: Option<&str>, count: usize, runner: &str) -> Result<usize, Error> {
    match value {
        Some("all") => Ok(count.max(1)),
        Some(value) => {
            let jobs = value.parse::<NonZeroU32>().map_err(|_| {
                Error::new(
                    codes::CLI_001,
                    format!("--arch-jobs must be a positive number or 'all', got '{value}'"),
                )
            })?;
            Ok((jobs.get() as usize).min(count.max(1)))
        }
        None if count <= 1 => Ok(1),
        None if runner == "qemu" => Ok(1),
        None => Ok(count.min(
            std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(1),
        )),
    }
}

fn step_jobs(jobs: NonZeroU32, arch_jobs: usize) -> NonZeroU32 {
    NonZeroU32::new((jobs.get() / arch_jobs.max(1) as u32).max(1))
        .expect("step job count is at least one")
}

/// Package an already-staged tree. This is the original `build --stage` mode.
fn package_stage(
    args: &BuildArgs,
    stage: &Path,
    output: &Path,
    pipeline_dir: Option<&Path>,
    policy: PlanPolicy<'_>,
    arch: &str,
) -> Result<Vec<PathBuf>, Error> {
    let signing = crate::commands::load_signing_key(
        args.signing.signing_key.as_deref(),
        args.signing.signing_passphrase_env.as_deref(),
    )?;
    let (bundle, plan) = load_plan(args.file.as_path(), arch, pipeline_dir, false, policy)?;
    if args.locked {
        crate::commands::lock::verify(
            &bundle,
            &plan,
            policy,
            args.policy.profile.as_deref(),
            arch,
            args.source_date_epoch,
            None,
            false,
        )?;
    }
    let source_date_epoch =
        crate::commands::source_date_epoch(&bundle.recipe, args.source_date_epoch);
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

    rpmx_core::output::phase(
        "plan",
        format!(
            "{} {}-{} · {arch}",
            cells[0].package, cells[0].version, cells[0].release
        ),
    );

    std::fs::create_dir_all(output).map_err(|error| {
        Error::new(
            codes::BUILD_001,
            format!("cannot create {}: {error}", output.display()),
        )
    })?;

    let mut written = Vec::new();
    for cell in &cells {
        let package_started = std::time::Instant::now();
        let packages = prepare_binary_packages(BinaryPackageOptions {
            recipe: &bundle.recipe,
            cell,
            stage,
            arch,
            source_date_epoch,
            debuginfo: false,
        })?;
        rpmx_core::output::phase(
            "package",
            format!(
                "{} · {}",
                arch,
                packages
                    .iter()
                    .map(|package| package.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        if !args.no_lint {
            let findings = crate::commands::package_lint::check(&packages);
            crate::commands::package_lint::enforce(&findings)?;
        }
        for package in packages {
            let path = emit::binary(&package, output, signing.as_ref(), signature_timestamp)?;
            rpmx_core::output::wrote(&path);
            written.push(path);
        }
        rpmx_core::output::phase(
            "package",
            format!(
                "{arch} · {} · {}",
                rpmx_core::output::count(cells.len(), "cell", "cells"),
                rpmx_core::output::duration(package_started)
            ),
        );
    }

    let mut repository = Vec::new();
    if !args.no_index {
        let report = crate::commands::repo::index(
            output,
            signing.as_ref(),
            signature_timestamp.unwrap_or(source_date_epoch),
        )?;
        repository.push(report.repomd);
        if let Some(signature) = report.signature {
            repository.push(signature);
        }
    }
    report(rpmx_core::output::json(), &written, &[], &[], &repository)?;
    Ok(written)
}

/// Run the SRPM-first pipeline for every requested architecture.
fn build_from_recipe(
    args: &BuildArgs,
    arches: &[String],
    pipeline_dir: Option<&Path>,
    policy: PlanPolicy<'_>,
    profile: Option<&Path>,
) -> Result<Vec<PathBuf>, Error> {
    let multi = arches.len() > 1;
    if multi {
        for (flag, set) in [
            ("--root", args.root.is_some()),
            ("--root-lock", args.root_lock.is_some()),
            ("--test-root-lock", args.test_root_lock.is_some()),
            ("--junit", args.junit.is_some()),
            ("--sbom", args.sbom.is_some()),
            ("--locked", args.locked),
        ] {
            if set {
                return Err(Error::new(
                    codes::BUILD_003,
                    format!("{flag} is not supported with multiple --arch values"),
                ));
            }
        }
    }

    let arch_jobs = resolve_arch_jobs(args.arch_jobs.as_deref(), arches.len(), &args.runner)?;
    let jobs = step_jobs(args.runtime.jobs, arch_jobs);
    let started = std::time::Instant::now();
    let outcomes = run_jobs(arches, arch_jobs, |arch| {
        rpmx_core::output::set_arch(Some(arch));
        let result = build_one(args, arch, pipeline_dir, policy, profile, jobs);
        rpmx_core::output::set_arch(None);
        result
    });

    let mut merged = ArchArtifacts::default();
    let mut errors = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(artifacts) => merged.merge(artifacts),
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        let code = errors[0].code;
        let message = errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(Error::new(code, message));
    }

    report(
        rpmx_core::output::json(),
        &merged.rpms,
        &merged.srpms,
        &merged.evidence,
        &merged.repository,
    )?;
    rpmx_core::output::phase(
        "done",
        format!(
            "{} in {} · {} architectures · {}",
            rpmx_core::output::count(merged.count(), "artifact", "artifacts"),
            rpmx_core::output::display_path(
                args.output
                    .as_deref()
                    .unwrap_or_else(|| Path::new("packages"))
            ),
            arches.len(),
            rpmx_core::output::duration(started)
        ),
    );
    Ok(merged.rpms)
}

fn run_jobs<F>(arches: &[String], limit: usize, run: F) -> Vec<Result<ArchArtifacts, Error>>
where
    F: Fn(&str) -> Result<ArchArtifacts, Error> + Sync + Send,
{
    if limit <= 1 {
        return arches.iter().map(|arch| run(arch)).collect();
    }
    let mut results: Vec<Option<Result<ArchArtifacts, Error>>> =
        (0..arches.len()).map(|_| None).collect();
    let queue = Mutex::new((0..arches.len()).collect::<Vec<_>>().into_iter());
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        let run = &run;
        for _ in 0..limit {
            let sender = sender.clone();
            let queue = &queue;
            scope.spawn(move || loop {
                let index = {
                    let mut queue = queue.lock().expect("architecture queue");
                    queue.next()
                };
                let Some(index) = index else { break };
                let arch = &arches[index];
                let result = catch_unwind(AssertUnwindSafe(|| run(arch))).unwrap_or_else(|_| {
                    Err(Error::new(
                        codes::BUILD_005,
                        format!("{arch} build panicked"),
                    ))
                });
                let _ = sender.send((index, result));
            });
        }
        drop(sender);
        for (index, result) in receiver {
            results[index] = Some(result);
        }
    });
    results
        .into_iter()
        .map(|result| result.expect("every architecture reports a result"))
        .collect()
}

fn build_one(
    args: &BuildArgs,
    arch: &str,
    pipeline_dir: Option<&Path>,
    policy: PlanPolicy<'_>,
    profile: Option<&Path>,
    jobs: NonZeroU32,
) -> Result<ArchArtifacts, Error> {
    let output = arch_output(args, arch);
    let build_started = std::time::Instant::now();
    let run_tests = args.run_tests || !args.test.is_empty();
    let (bundle, plan) = load_plan(&args.file, arch, pipeline_dir, false, policy)?;
    if !args.test.is_empty() {
        let known: std::collections::BTreeSet<_> = plan
            .cells
            .iter()
            .flat_map(|cell| cell.tests.iter().map(|test| test.name.as_str()))
            .collect();
        for name in &args.test {
            if !known.contains(name.as_str()) {
                return Err(Error::new(
                    codes::TEST_004,
                    format!("no test named '{name}' in this recipe"),
                ));
            }
        }
    }
    if args.locked {
        crate::commands::lock::verify(
            &bundle,
            &plan,
            policy,
            profile,
            arch,
            args.source_date_epoch,
            None,
            false,
        )?;
    }
    let automatic = if args.root_lock.is_none() && args.root.is_none() && !args.unsafe_host_root {
        crate::commands::autoroot::ensure(
            &bundle.recipe,
            &plan,
            &bundle.root,
            profile,
            args.runtime.cache.cache.as_deref(),
            args.offline,
            false,
            arch,
        )?
    } else {
        None
    };
    let verify_digest = automatic.is_none();
    // A cross build runs its steps natively in the host-architecture build
    // root, with the target root mounted as a sysroot.
    let host_arch = rpmx_core::arch::Arch::current()?;
    let target_arch = rpmx_core::arch::Arch::parse(arch)?;
    let sysroot: Option<std::path::PathBuf> = automatic
        .as_ref()
        .and_then(|root| root.sysroot.as_ref())
        .map(|sysroot| sysroot.root.clone());
    let cross = automatic.as_ref().and_then(|root| root.cross.clone());
    let sysroot_packages: Option<std::collections::BTreeSet<String>> = automatic
        .as_ref()
        .and_then(|root| root.sysroot.as_ref())
        .map(|sysroot| {
            sysroot
                .document
                .packages
                .values()
                .flatten()
                .map(|package| package.name.clone())
                .collect()
        });
    let exec = if sysroot.is_some() {
        host_arch
    } else {
        target_arch
    };
    let (root_lock, root) = match automatic {
        Some(root) => {
            crate::commands::autoroot::report_root(
                &root.document,
                &root.profile,
                if root.cached {
                    "cache hit"
                } else {
                    "assembled"
                },
            );
            (Some(root.lock), Some(root.root))
        }
        None => (args.root_lock.clone(), args.root.clone()),
    };
    if args.locked {
        let active_lock = root_lock
            .as_deref()
            .map(rpmx_core::root::load_lock)
            .transpose()?;
        crate::commands::lock::verify(
            &bundle,
            &plan,
            policy,
            profile,
            arch,
            args.source_date_epoch,
            active_lock.as_ref(),
            true,
        )?;
    }
    let _root_request = rpmx_core::request::resolve(rpmx_core::request::RootRequestOptions {
        lock: root_lock.as_deref(),
        root: root.as_deref(),
        allow_host: args.unsafe_host_root,
        arch: exec.as_str(),
        verify_digest,
    })?;

    let selected: Vec<_> = match args.cell.as_deref() {
        Some(id) => plan.cells.iter().filter(|cell| cell.id == id).collect(),
        None => plan.cells.iter().collect(),
    };
    if selected.is_empty() {
        return Err(Error::new(
            codes::BUILD_002,
            format!(
                "no such cell '{}'",
                args.cell.as_deref().unwrap_or_default()
            ),
        ));
    }
    if selected.len() == 1 {
        rpmx_core::output::phase(
            "plan",
            format!(
                "{} {}-{} · {arch}",
                selected[0].package, selected[0].version, selected[0].release
            ),
        );
    } else {
        rpmx_core::output::phase("plan", format!("{} cells · {arch}", selected.len()));
    }

    let pipeline = BuildPipeline {
        file: &bundle.root,
        output: &output,
        arch,
        exec,
        sysroot: sysroot.as_deref(),
        cross: cross.as_ref(),
        sysroot_packages: sysroot_packages.as_ref(),
        runner: &args.runner,
        policy,
        profile,
        cache: args.runtime.cache.cache.as_deref(),
        proxy: args.proxy.as_deref(),
        offline: args.offline,
        root_lock: root_lock.as_deref(),
        root: root.as_deref(),
        runner_asset_root: args.runner_asset_root.as_deref(),
        unsafe_host_root: args.unsafe_host_root,
        source_date_epoch: args.source_date_epoch,
        jobs,
        step_timeout: args.runtime.step_timeout,
        run_tests,
        test_filter: &args.test,
        junit: args.junit.as_deref(),
        no_lint: args.no_lint,
        sbom: args.sbom.as_deref(),
        test_root_lock: args.test_root_lock.as_deref(),
        signing: &args.signing,
        pipeline_dir,
    };
    let built = pipeline.execute(&selected)?;
    let mut repository = Vec::new();
    if !args.no_index {
        let timestamp = crate::commands::source_date_epoch(&bundle.recipe, args.source_date_epoch);
        let key = crate::commands::load_signing_key(
            args.signing.signing_key.as_deref(),
            args.signing.signing_passphrase_env.as_deref(),
        )?;
        let signature_timestamp = crate::commands::signature_timestamp(
            args.signing.signing_timestamp,
            timestamp,
            key.as_ref(),
        )?;
        let report = crate::commands::repo::index(
            &output,
            key.as_ref(),
            signature_timestamp.unwrap_or(timestamp),
        )?;
        rpmx_core::output::phase(
            "index",
            format!(
                "{} in {}",
                rpmx_core::output::count(report.packages, "package", "packages"),
                rpmx_core::output::display_path(&output)
            ),
        );
        repository.push(report.repomd);
        if let Some(signature) = report.signature {
            repository.push(signature);
        }
    }
    let artifacts = ArchArtifacts {
        rpms: built.rpms,
        srpms: built.srpms,
        evidence: built.evidence,
        repository,
    };
    rpmx_core::output::phase(
        "done",
        format!(
            "{} in {} · {}",
            rpmx_core::output::count(artifacts.count(), "artifact", "artifacts"),
            rpmx_core::output::display_path(&output),
            rpmx_core::output::duration(build_started)
        ),
    );
    Ok(artifacts)
}

fn report(
    json: bool,
    rpms: &[PathBuf],
    srpms: &[PathBuf],
    evidence: &[PathBuf],
    repository: &[PathBuf],
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
            crate::commands::report::ArtifactGroup {
                kind: "repository",
                paths: repository,
            },
        ],
        None,
    )?;
    Ok(rpms.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn output_is_always_arch_scoped() {
        assert_eq!(
            output_path(None, "aarch64"),
            PathBuf::from("packages/aarch64")
        );
        assert_eq!(
            output_path(Some(Path::new("/tmp/out")), "x86_64"),
            PathBuf::from("/tmp/out/x86_64")
        );
    }

    #[test]
    fn arch_jobs_defaults_and_bounds() {
        assert_eq!(resolve_arch_jobs(None, 1, "auto").expect("single"), 1);
        assert_eq!(resolve_arch_jobs(None, 4, "qemu").expect("qemu"), 1);
        assert_eq!(resolve_arch_jobs(Some("all"), 4, "qemu").expect("all"), 4);
        assert_eq!(resolve_arch_jobs(Some("2"), 4, "auto").expect("two"), 2);
        assert_eq!(resolve_arch_jobs(Some("9"), 4, "auto").expect("cap"), 4);
        assert!(resolve_arch_jobs(Some("0"), 4, "auto").is_err());
        assert!(resolve_arch_jobs(Some("0"), 1, "auto").is_err());
        assert!(resolve_arch_jobs(Some("nope"), 4, "auto").is_err());
    }

    #[test]
    fn step_jobs_are_divided_by_active_architectures() {
        assert_eq!(
            step_jobs(NonZeroU32::new(8).unwrap(), 2),
            NonZeroU32::new(4).unwrap()
        );
        assert_eq!(
            step_jobs(NonZeroU32::new(1).unwrap(), 4),
            NonZeroU32::new(1).unwrap()
        );
        assert_eq!(
            step_jobs(NonZeroU32::new(8).unwrap(), 1),
            NonZeroU32::new(8).unwrap()
        );
    }

    #[test]
    fn the_scheduler_respects_its_limit_and_preserves_order() {
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let arches = ["a", "b", "c", "d"].map(String::from);
        let outcomes = run_jobs(&arches, 2, |arch| {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            active.fetch_sub(1, Ordering::SeqCst);
            let mut artifacts = ArchArtifacts::default();
            artifacts.rpms.push(PathBuf::from(arch));
            Ok(artifacts)
        });

        assert!(peak.load(Ordering::SeqCst) <= 2, "limit exceeded");
        let order: Vec<_> = outcomes
            .into_iter()
            .map(|outcome| outcome.expect("build").rpms[0].clone())
            .collect();
        assert_eq!(order, arches.iter().map(PathBuf::from).collect::<Vec<_>>());
    }

    #[test]
    fn a_failure_does_not_stop_other_architectures() {
        let ran = AtomicUsize::new(0);
        let arches = ["a", "b", "c"].map(String::from);
        let outcomes = run_jobs(&arches, 2, |arch| {
            ran.fetch_add(1, Ordering::SeqCst);
            if arch == "b" {
                Err(Error::new(codes::BUILD_005, "bad arch"))
            } else {
                Ok(ArchArtifacts::default())
            }
        });
        assert_eq!(ran.load(Ordering::SeqCst), 3);
        assert!(outcomes[0].is_ok());
        assert!(outcomes[1].is_err());
        assert!(outcomes[2].is_ok());
    }

    #[test]
    fn a_panicking_worker_is_contained() {
        let arches = ["a", "b"].map(String::from);
        let outcomes = run_jobs(&arches, 2, |arch| {
            if arch == "b" {
                panic!("boom");
            }
            Ok(ArchArtifacts::default())
        });
        assert!(outcomes[0].is_ok());
        assert_eq!(
            outcomes[1].as_ref().expect_err("panic").code,
            codes::BUILD_005
        );
    }
}
