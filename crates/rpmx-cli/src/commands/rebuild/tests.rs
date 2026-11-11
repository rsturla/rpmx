use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::plan::PlannedCell;
use rpmx_core::root::RootLock;
use rpmx_core::runner::RunnerKind;
use rpmx_core::sandbox::Sandbox;
use rpmx_core::test_root::{prepare as prepare_test_root, TestRootOptions};
use rpmx_core::test_runner::{run_planned_tests, TestSandboxConfig};
use rpmx_core::Error;

use crate::commands::report::{self, TestOutcome};
use crate::commands::test_roots::{
    filter_tests, grouped, key_for, read_rpms, rpms_for_output, under_test_repo,
};

pub(super) struct TestRunOptions<'a> {
    pub(super) sandbox: &'a Sandbox<'a>,
    pub(super) cell: &'a PlannedCell,
    pub(super) written: &'a [PathBuf],
    pub(super) output: &'a Path,
    pub(super) work: &'a Path,
    pub(super) build: &'a Path,
    pub(super) stage: &'a Path,
    pub(super) fixture_root: &'a Path,
    pub(super) active_lock: Option<&'a RootLock>,
    /// Target sysroot lock for a cross build; installed tests run there.
    pub(super) sysroot_lock: Option<&'a RootLock>,
    pub(super) builder_image: Option<&'a str>,
    pub(super) pinned_lock: Option<&'a Path>,
    pub(super) arch: &'a str,
    pub(super) runner: RunnerKind,
    pub(super) appliance: Option<&'a Path>,
    pub(super) source_date_epoch: u32,
    pub(super) jobs: NonZeroU32,
    pub(super) timeout: NonZeroU64,
    /// Skip installed-stage tests, which execute target binaries.
    pub(super) skip_installed: bool,
    /// Keep the prepared test root here instead of deleting it.
    pub(super) save_test_root: Option<&'a Path>,
    pub(super) filter: &'a [String],
    pub(super) junit: Option<&'a Path>,
}

/// Run build- and installed-stage tests and persist the test-root lock.
pub(super) fn run(options: TestRunOptions<'_>) -> Result<Vec<PathBuf>, Error> {
    let junit = options.junit.map(Path::to_path_buf);
    let result = run_inner(options);
    if let (Err(error), Some(path)) = (&result, junit) {
        if error.code != codes::TEST_003 {
            report::write_junit(&path, &[TestOutcome::setup(error)])
                .map_err(|write| Error::new(codes::TEST_004, write.to_string()))?;
        }
    }
    result
}

fn run_inner(options: TestRunOptions<'_>) -> Result<Vec<PathBuf>, Error> {
    let candidates: Vec<_> = options
        .cell
        .tests
        .iter()
        .filter(|test| !options.skip_installed || test.stage == "build")
        .cloned()
        .collect();
    let tests = filter_tests(&candidates, options.filter)?;
    if !options.filter.is_empty() && tests.is_empty() {
        // Another matrix cell may own the selected test.
        return Ok(Vec::new());
    }
    let cache = crate::commands::fetch::default_cache()?;
    let groups = grouped(&tests);
    if groups.len() > 1 && options.save_test_root.is_some() {
        return Err(Error::new(
            codes::TEST_004,
            "this recipe has multiple installed-test environments; --save-test-root needs one (pass --test NAME)",
        ));
    }
    if groups.len() > 1 && options.pinned_lock.is_some() {
        return Err(Error::new(
            codes::TEST_004,
            "this recipe has multiple installed-test environments; --test-root-lock needs one (pass --test NAME)",
        ));
    }
    let mut roots = BTreeMap::new();
    if !groups.is_empty() {
        // A cross build's target root is the sysroot, not the host build root.
        let lock = options
            .sysroot_lock
            .or(options.active_lock)
            .ok_or_else(|| {
                Error::new(
                    codes::TEST_005,
                    "installed-stage tests need an embedded or explicit root lock",
                )
            })?;
        let test_rpms = read_rpms(options.written)?;
        for (index, group) in groups.iter().enumerate() {
            let work = options.work.join(format!("test-root-{index}"));
            let group_rpms = rpms_for_output(group.output.as_deref(), &test_rpms, &group.packages)?;
            let repo = under_test_repo(&work.join("test-rpms"), &group_rpms)?;
            let prepared = prepare_test_root(TestRootOptions {
                lock,
                destination: if groups.len() == 1 {
                    options.save_test_root
                } else {
                    None
                },
                pinned: if groups.len() == 1 {
                    options.pinned_lock
                } else {
                    None
                },
                tools: &group.packages,
                rpms: &group_rpms,
                repo: &repo,
                image: options.builder_image,
                cache: &cache,
                work: &work,
            })?;
            roots.insert(group.key.clone(), prepared);
        }
    }
    let prefix = options
        .active_lock
        .map(|lock| lock.prefix.as_str())
        .unwrap_or(rpmx_core::guest::PREFIX);
    let libdir = options
        .active_lock
        .map(|lock| lock.libdir.as_str())
        .unwrap_or(rpmx_core::guest::LIBDIR);
    let settings = TestSandboxConfig {
        workspace: options.build,
        stage: options.stage,
        fixture_root: options.fixture_root,
        results: &options.output.join("test-results"),
        rpms: options.written,
        host_arch: rpmx_core::arch::Arch::current()?,
        target_arch: rpmx_core::arch::Arch::parse(options.arch)?,
        runner: options.runner,
        assets: rpmx_core::runner::RunnerAssets::new(options.appliance, &[]),
        source_date_epoch: options.source_date_epoch,
        jobs: options.jobs,
        timeout: options.timeout,
        prefix,
        libdir,
    };
    let mut outcomes = Vec::new();
    for test in &tests {
        let test_root = if test.stage != "build" {
            roots.get(&key_for(test)).map(|root| root.path.as_path())
        } else {
            None
        };
        let started = std::time::Instant::now();
        let result = run_planned_tests(
            options.sandbox,
            std::slice::from_ref(test),
            test_root,
            settings,
        );
        let error = result.as_ref().err().map(|error| error.message.clone());
        if let Err(error) = &result {
            eprintln!("rpmx: {}: {}", test.qualified_name(), error);
        }
        eprintln!(
            "rpmx: test {} ({}): {} ({:.1}s)",
            test.qualified_name(),
            test.stage,
            if error.is_some() { "FAILED" } else { "ok" },
            started.elapsed().as_secs_f64()
        );
        outcomes.push(TestOutcome::new(
            test,
            &result,
            started.elapsed(),
            settings.results,
        ));
    }
    let failed = outcomes
        .iter()
        .filter(|outcome| outcome.error.is_some())
        .count();
    if let Some(path) = options.junit {
        report::write_junit(path, &outcomes).map_err(|error| {
            Error::new(
                codes::TEST_003,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }
    let mut evidence = Vec::new();
    for (index, prepared) in roots.values().enumerate() {
        if options.save_test_root.is_none() {
            let _ = rpmx_core::root::remove_tree(&prepared.path);
        }
        let suffix = if index == 0 {
            String::new()
        } else {
            format!(".{index}")
        };
        let path = options.output.join(format!(
            "{}-{}-{}{suffix}.test-root.lock.json",
            options.cell.package, options.cell.version, options.cell.release
        ));
        let text = serde_yaml_ng::to_string(&prepared.lock)
            .map_err(|error| Error::new(codes::TEST_003, error.to_string()))?;
        std::fs::write(&path, text).map_err(|error| {
            Error::new(
                codes::TEST_003,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
        evidence.push(path);
    }
    if failed > 0 {
        return Err(Error::new(
            codes::TEST_003,
            format!("{failed} test(s) failed"),
        ));
    }
    Ok(evidence)
}
