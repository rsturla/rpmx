use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::Path;

use crate::runner::{RunnerAssets, RunnerKind};
use crate::sandbox::{Sandbox, SandboxConfig};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};
use rpmx_language::plan::PlannedStep;

#[derive(Clone, Copy)]
pub struct TestSandboxConfig<'a> {
    pub workspace: &'a Path,
    pub stage: &'a Path,
    pub host_arch: Arch,
    pub target_arch: Arch,
    pub runner: RunnerKind,
    pub assets: RunnerAssets<'a>,
    pub source_date_epoch: u32,
    pub jobs: NonZeroU32,
    pub timeout: NonZeroU64,
    pub prefix: &'a str,
    pub libdir: &'a str,
    pub debuginfo: bool,
}

/// Run build-stage tests in the build sandbox, then installed-stage tests in a
/// separate test root.
pub fn run_planned_tests(
    sandbox: &Sandbox<'_>,
    tests: &[rpmx_language::plan::PlannedTest],
    test_root: Option<&Path>,
    settings: TestSandboxConfig<'_>,
) -> Result<()> {
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
            codes::TEST_004,
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
            codes::TEST_005,
            "installed-stage tests need a test root, which requires a root lock",
        )
    })?;
    let test_sandbox = Sandbox::new(SandboxConfig {
        runner: settings.runner,
        host: settings.host_arch,
        target: settings.target_arch,
        workspace: settings.workspace,
        stage: settings.stage,
        root: Some(root),
        assets: settings.assets,
        image: None,
        source_date_epoch: settings.source_date_epoch,
        jobs: settings.jobs,
        timeout: settings.timeout,
        prefix: settings.prefix,
        libdir: settings.libdir,
        debuginfo: settings.debuginfo,
    })?;
    for test in installed_tests {
        run_one_test(&test_sandbox, test)?;
    }
    Ok(())
}

fn run_one_test(sandbox: &Sandbox<'_>, test: &rpmx_language::plan::PlannedTest) -> Result<()> {
    let _phase = rpmx_base::output::enter_phase("test");
    rpmx_base::output::phase("test", format!("{} ({})", test.name, test.stage));
    let total = test.pipeline.len() + usize::from(test.argv.is_some());
    let mut position = 0;
    if let Some(argv) = &test.argv {
        let command = argv.first().ok_or_else(|| {
            Error::new(
                codes::TEST_004,
                format!("test '{}' has an empty argv", test.name),
            )
        })?;
        let step = PlannedStep {
            label: format!("test:{}", test.name),
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
        position += 1;
        sandbox
            .run_numbered(&step, position, total)
            .map_err(test_failure)?;
    }
    for step in &test.pipeline {
        position += 1;
        sandbox
            .run_numbered(step, position, total)
            .map_err(test_failure)?;
    }
    Ok(())
}

/// A step failure inside a test is a test failure, not a build failure.
fn test_failure(error: Error) -> Error {
    error.with_code(codes::TEST_003)
}
