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
}

/// Run build- and installed-stage tests and persist the test-root lock.
pub(super) fn run(options: TestRunOptions<'_>) -> Result<(), Error> {
    let cache = crate::commands::fetch::default_cache()?;
    let prepared = if options
        .cell
        .tests
        .iter()
        .any(|test| test.stage == "installed")
    {
        let lock = options.active_lock.ok_or_else(|| {
            Error::new(
                codes::BUILD_007,
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
    let result = run_planned_tests(
        options.sandbox,
        &options.cell.tests,
        test_root,
        TestSandboxConfig {
            workspace: options.build,
            stage: options.stage,
            host_arch: rpmx_core::arch::Arch::current()?,
            target_arch: rpmx_core::arch::Arch::parse(options.arch)?,
            runner: options.runner,
            source_date_epoch: options.source_date_epoch,
            jobs: options.jobs,
            timeout: options.timeout,
            debuginfo: options.debuginfo,
        },
    );
    if let Some(prepared) = prepared {
        let _ = rpmx_core::root::remove_tree(&prepared.path);
        let path = options.output.join(format!(
            "{}-{}-{}.test-root.lock.json",
            options.cell.package, options.cell.version, options.cell.release
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
    result
}
