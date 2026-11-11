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
    pub results: &'a Path,
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
    let build_tests: Vec<_> = tests
        .iter()
        .filter(|test| test.stage == "build" && test.skip.is_none())
        .collect();
    let installed_tests: Vec<_> = tests
        .iter()
        .filter(|test| test.stage == "installed" && test.skip.is_none())
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
            let result = run_one_test(sandbox, &test, settings.results);
            sandbox.sync()?;
            collect_artifacts(
                &test,
                settings.workspace,
                &result_dir(settings.results, &test),
            )?;
            if rpmx_base::output::keep_workdir() {
                rpmx_base::output::wrote(&files.keep());
            }
            result?;
        } else {
            let result = run_one_test(sandbox, test, settings.results);
            sandbox.sync()?;
            collect_artifacts(
                test,
                settings.workspace,
                &result_dir(settings.results, test),
            )?;
            result?;
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
        let result = run_one_test(&test_sandbox, &test, settings.results);
        test_sandbox.sync()?;
        collect_artifacts(&test, &workspace, &result_dir(settings.results, &test))?;
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

pub fn result_dir(base: &Path, test: &rpmx_language::PlannedTest) -> std::path::PathBuf {
    let digest = rpmx_base::hash::sha512_hex(test.qualified_name().as_bytes());
    base.join(&digest.trim_start_matches("sha512:")[..24])
}

fn collect_artifacts(
    test: &rpmx_language::PlannedTest,
    workspace: &Path,
    results: &Path,
) -> Result<()> {
    for artifact in &test.artifacts {
        let path = std::path::Path::new(artifact);
        if path.components().count() == 0
            || path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(Error::new(
                codes::TEST_004,
                "test artifacts must be relative workspace paths",
            ));
        }
        let source = workspace.join(path);
        if !source.exists() {
            continue;
        }
        let mut parent = workspace.to_path_buf();
        for part in path.components() {
            parent.push(part);
            if std::fs::symlink_metadata(&parent)
                .map_err(file_error)?
                .file_type()
                .is_symlink()
            {
                return Err(Error::new(
                    codes::TEST_004,
                    "test artifacts cannot contain symlinks",
                ));
            }
        }
        let files = if source.is_dir() {
            rpmx_language::test_files::entries(workspace, artifact)?
        } else {
            vec![source]
        };
        for file in files {
            let target = results
                .join("artifacts")
                .join(file.strip_prefix(workspace).expect("artifact in workspace"));
            std::fs::create_dir_all(target.parent().expect("artifact directory"))
                .map_err(file_error)?;
            std::fs::copy(file, target).map_err(file_error)?;
        }
    }
    Ok(())
}

fn run_one_test(
    sandbox: &Sandbox<'_>,
    test: &rpmx_language::plan::PlannedTest,
    results: &Path,
) -> Result<()> {
    let directory = result_dir(results, test);
    if directory.exists() {
        std::fs::remove_dir_all(&directory).map_err(file_error)?;
    }
    std::fs::create_dir_all(&directory).map_err(file_error)?;
    let logs = crate::runner::OutputLogs {
        stdout: std::fs::File::create(directory.join("stdout.log")).map_err(file_error)?,
        stderr: std::fs::File::create(directory.join("stderr.log")).map_err(file_error)?,
    };
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
            .run_recorded(&step, position, total, &logs)
            .map_err(test_failure)?;
    }
    for step in &test.pipeline {
        position += 1;
        let mut step = step.clone();
        for (key, value) in &test.environment {
            step.env.entry(key.clone()).or_insert_with(|| value.clone());
        }
        sandbox
            .run_recorded(&step, position, total, &logs)
            .map_err(test_failure)?;
    }
    Ok(())
}

/// A step failure inside a test is a test failure, not a build failure.
fn test_failure(error: Error) -> Error {
    if error.code == codes::SANDBOX_002 || error.code == codes::SANDBOX_004 {
        error.with_code(codes::TEST_003)
    } else {
        error
    }
}
