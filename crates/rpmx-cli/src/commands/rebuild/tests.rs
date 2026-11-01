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

pub(super) struct TestRunOptions<'a> {
    pub(super) sandbox: &'a Sandbox<'a>,
    pub(super) cell: &'a PlannedCell,
    pub(super) written: &'a [PathBuf],
    pub(super) output: &'a Path,
    pub(super) work: &'a Path,
    pub(super) build: &'a Path,
    pub(super) stage: &'a Path,
    pub(super) active_lock: Option<&'a RootLock>,
    pub(super) builder_image: Option<&'a str>,
    pub(super) pinned_lock: Option<&'a Path>,
    pub(super) arch: &'a str,
    pub(super) runner: RunnerKind,
    pub(super) source_date_epoch: u32,
    pub(super) jobs: NonZeroU32,
    pub(super) timeout: NonZeroU64,
    pub(super) debuginfo: bool,
    pub(super) filter: &'a [String],
    pub(super) junit: Option<&'a Path>,
}

/// Run build- and installed-stage tests and persist the test-root lock.
pub(super) fn run(options: TestRunOptions<'_>) -> Result<Option<PathBuf>, Error> {
    let tests: Vec<_> = options
        .cell
        .tests
        .iter()
        .filter(|test| options.filter.is_empty() || options.filter.contains(&test.name))
        .cloned()
        .collect();
    if !options.filter.is_empty() && tests.is_empty() {
        // Another matrix cell may own the selected test.
        return Ok(None);
    }
    let cache = crate::commands::fetch::default_cache()?;
    let prepared = if tests.iter().any(|test| test.stage == "installed") {
        let lock = options.active_lock.ok_or_else(|| {
            Error::new(
                codes::TEST_005,
                "installed-stage tests need an embedded or explicit root lock",
            )
        })?;
        Some(prepare_test_root(TestRootOptions {
            lock,
            pinned: options.pinned_lock,
            tools: &options.cell.test_tools,
            rpms: options.written,
            image: options.builder_image,
            cache: &cache,
            work: options.work,
            runner: options.runner,
        })?)
    } else {
        None
    };
    let test_root = prepared.as_ref().map(|root| root.path.as_path());
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
        host_arch: rpmx_core::arch::Arch::current()?,
        target_arch: rpmx_core::arch::Arch::parse(options.arch)?,
        runner: options.runner,
        source_date_epoch: options.source_date_epoch,
        jobs: options.jobs,
        timeout: options.timeout,
        prefix,
        libdir,
        debuginfo: options.debuginfo,
    };
    let mut outcomes = Vec::new();
    for test in &tests {
        let started = std::time::Instant::now();
        let result = run_planned_tests(
            options.sandbox,
            std::slice::from_ref(test),
            test_root,
            settings,
        );
        let error = result.as_ref().err().map(|error| error.message.clone());
        eprintln!(
            "rpmx: test {} ({}): {} ({:.1}s)",
            test.name,
            test.stage,
            if error.is_some() { "FAILED" } else { "ok" },
            started.elapsed().as_secs_f64()
        );
        outcomes.push(TestOutcome {
            name: test.name.clone(),
            stage: test.stage.clone(),
            error,
            duration: started.elapsed(),
        });
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
    let evidence = if let Some(prepared) = prepared {
        let _ = rpmx_core::root::remove_tree(&prepared.path);
        let path = options.output.join(format!(
            "{}-{}-{}.test-root.lock.json",
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
        Some(path)
    } else {
        None
    };
    if failed > 0 {
        return Err(Error::new(
            codes::TEST_003,
            format!("{failed} test(s) failed"),
        ));
    }
    let _ = evidence;
    Ok(evidence)
}
