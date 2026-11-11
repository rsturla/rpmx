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
    pub fixture_root: &'a Path,
    pub host_arch: Arch,
    pub target_arch: Arch,
    pub runner: RunnerKind,
    pub assets: RunnerAssets<'a>,
    pub source_date_epoch: u32,
    pub jobs: NonZeroU32,
    pub timeout: NonZeroU64,
    pub prefix: &'a str,
    pub libdir: &'a str,
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
                test.qualified_name(),
                test.stage
            ),
        ));
    }

    for test in build_tests {
        if test.files.is_some() {
            sandbox.restart()?;
            let files = tempfile::Builder::new()
                .prefix(".rpmx-test-")
                .tempdir_in(settings.workspace)
                .map_err(file_error)?;
            let mut test = test.clone();
            prepare_files(
                &mut test,
                settings.fixture_root,
                files.path(),
                settings.workspace,
            )?;
            let result = run_one_test(sandbox, &test);
            sandbox.sync()?;
            if rpmx_base::output::keep_workdir() {
                rpmx_base::output::wrote(&files.keep());
            }
            result?;
        } else {
            run_one_test(sandbox, test)?;
        }
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
    for test in installed_tests {
        let work = tempfile::tempdir().map_err(file_error)?;
        let workspace = work.path().join("build");
        let stage = work.path().join("stage");
        std::fs::create_dir_all(&workspace).map_err(file_error)?;
        std::fs::create_dir_all(&stage).map_err(file_error)?;
        let mut test = test.clone();
        prepare_files(
            &mut test,
            settings.fixture_root,
            &workspace.join("files"),
            &workspace,
        )?;
        let test_sandbox = Sandbox::new(SandboxConfig {
            runner: settings.runner,
            host: settings.host_arch,
            exec: settings.target_arch,
            target: settings.target_arch,
            sysroot: None,
            cross: None,
            workspace: &workspace,
            stage: &stage,
            root: Some(root),
            assets: settings.assets,
            image: None,
            source_date_epoch: settings.source_date_epoch,
            jobs: settings.jobs,
            timeout: settings.timeout,
            prefix: settings.prefix,
            libdir: settings.libdir,
        })?;
        let result = run_one_test(&test_sandbox, &test);
        test_sandbox.sync()?;
        if rpmx_base::output::keep_workdir() {
            rpmx_base::output::wrote(&work.keep());
        }
        result?;
    }
    Ok(())
}

fn file_error(error: std::io::Error) -> Error {
    Error::new(
        codes::TEST_004,
        format!("cannot prepare test workspace: {error}"),
    )
}

fn prepare_files(
    test: &mut rpmx_language::PlannedTest,
    recipe_dir: &Path,
    destination: &Path,
    workspace: &Path,
) -> Result<()> {
    let Some(directory) = &test.files else {
        return Ok(());
    };
    std::fs::create_dir_all(destination).map_err(file_error)?;
    let source = recipe_dir.join(directory);
    for path in rpmx_language::test_files::entries(recipe_dir, directory)? {
        let target = destination.join(
            path.strip_prefix(&source)
                .expect("file below fixture directory"),
        );
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(file_error)?;
        }
        std::fs::copy(path, target).map_err(file_error)?;
    }
    let relative = destination
        .strip_prefix(workspace)
        .expect("files in test workspace");
    test.environment.insert(
        "TEST_FILES".into(),
        format!("{}/{}", rpmx_base::guest::BUILD, relative.display()),
    );
    Ok(())
}

fn run_one_test(sandbox: &Sandbox<'_>, test: &rpmx_language::plan::PlannedTest) -> Result<()> {
    let _phase = rpmx_base::output::enter_phase("test");
    rpmx_base::output::phase(
        "test",
        format!("{} ({})", test.qualified_name(), test.stage),
    );
    let total = test.pipeline.len() + usize::from(test.argv.is_some());
    let mut position = 0;
    if let Some(argv) = &test.argv {
        let command = argv.first().ok_or_else(|| {
            Error::new(
                codes::TEST_004,
                format!("test '{}' has an empty argv", test.qualified_name()),
            )
        })?;
        let step = PlannedStep {
            label: format!("test:{}", test.qualified_name()),
            uses: format!("test:{}", test.qualified_name()),
            command: Some(command.clone()),
            args: argv[1..].to_vec(),
            cwd: None,
            env: test.environment.clone(),
            needs: Vec::new(),
            needs_packages: Vec::new(),
            needs_sysroot: Vec::new(),
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
        let mut step = step.clone();
        for (key, value) in &test.environment {
            step.env.entry(key.clone()).or_insert_with(|| value.clone());
        }
        sandbox
            .run_numbered(&step, position, total)
            .map_err(test_failure)?;
    }
    Ok(())
}

/// A step failure inside a test is a test failure, not a build failure.
fn test_failure(error: Error) -> Error {
    error.with_code(codes::TEST_003)
}
