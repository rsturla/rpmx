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
    pub rpms: &'a [std::path::PathBuf],
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
        .filter(|test| test.stage != "build" && test.skip.is_none())
        .collect();
    if let Some(test) = tests
        .iter()
        .find(|test| !matches!(test.stage.as_str(), "build" | "installed" | "transaction"))
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
            let artifacts = collect_artifacts(
                &test,
                settings.workspace,
                &result_dir(settings.results, &test),
            );
            if rpmx_base::output::keep_workdir() {
                rpmx_base::output::wrote(&files.keep());
            }
            finish_test(result, artifacts)?;
        } else {
            let result = run_one_test(sandbox, test, settings.results);
            sandbox.sync()?;
            let artifacts = collect_artifacts(
                test,
                settings.workspace,
                &result_dir(settings.results, test),
            );
            finish_test(result, artifacts)?;
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
        let transaction = test.stage == "transaction";
        let private_root = work.path().join("root");
        let _cleanup = TransactionRootCleanup(private_root.clone());
        if transaction {
            if settings.rpms.is_empty() {
                return Err(Error::new(
                    codes::TEST_004,
                    "transaction tests need RPMs; pass --install-dir with a saved root",
                ));
            }
            copy_transaction_root(root, &private_root)?;
        }
        let mut test = test.clone();
        prepare_files(
            &mut test,
            settings.fixture_root,
            &workspace.join("files"),
            &workspace,
        )?;
        if !settings.rpms.is_empty() {
            let directory = workspace.join("rpms");
            std::fs::create_dir_all(&directory).map_err(file_error)?;
            for rpm in settings.rpms {
                std::fs::copy(
                    rpm,
                    directory.join(
                        rpm.file_name().ok_or_else(|| {
                            Error::new(codes::TEST_004, "test RPM has no file name")
                        })?,
                    ),
                )
                .map_err(file_error)?;
            }
            test.environment.insert(
                "TEST_RPMS".into(),
                format!("{}/rpms", rpmx_base::guest::BUILD),
            );
        }
        let test_sandbox = Sandbox::new(SandboxConfig {
            writable_root: transaction,
            runner: settings.runner,
            host: settings.host_arch,
            exec: settings.target_arch,
            target: settings.target_arch,
            sysroot: None,
            cross: None,
            workspace: &workspace,
            stage: &stage,
            root: Some(if transaction { &private_root } else { root }),
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
        let artifacts = collect_artifacts(&test, &workspace, &result_dir(settings.results, &test));
        if rpmx_base::output::keep_workdir() {
            rpmx_base::output::wrote(&work.keep());
        }
        finish_test(result, artifacts)?;
    }
    Ok(())
}

fn copy_transaction_root(source: &Path, destination: &Path) -> Result<()> {
    let source = std::fs::canonicalize(source).map_err(file_error)?;
    if source == Path::new("/") || destination.starts_with(&source) {
        return Err(Error::new(
            codes::TEST_004,
            "transaction tests need a disposable package root, not the host root",
        ));
    }
    std::fs::create_dir_all(destination).map_err(file_error)?;
    let output = std::process::Command::new("unshare")
        .args(["--user", "--map-root-user", "--", "cp"])
        .args(["--archive", "--reflink=auto", "--no-preserve=ownership"])
        .arg(source.join("."))
        .arg(destination)
        .output()
        .map_err(file_error)?;
    if !output.status.success() {
        return Err(Error::new(
            codes::TEST_004,
            format!(
                "cannot copy transaction root: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    Ok(())
}

struct TransactionRootCleanup(std::path::PathBuf);

impl Drop for TransactionRootCleanup {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            let Ok(metadata) = std::fs::symlink_metadata(path) else {
                return;
            };
            if !metadata.is_dir() {
                return;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    path,
                    std::fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
                );
            }
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    writable(&entry.path());
                }
            }
        }
        writable(&self.0);
    }
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
    // Frame the cell separately so separators in user-provided names cannot
    // make two different (cell, test) pairs share the same identity.
    let identity = format!(
        "{}:{}{}",
        test.cell_id.len(),
        test.cell_id,
        test.qualified_name()
    );
    let digest = rpmx_base::hash::sha512_hex(identity.as_bytes());
    base.join(&digest.trim_start_matches("sha512:")[..24])
}

fn collect_artifacts(
    test: &rpmx_language::PlannedTest,
    workspace: &Path,
    results: &Path,
) -> Result<()> {
    let workspace = std::fs::canonicalize(workspace).map_err(file_error)?;
    for artifact in &test.artifacts {
        let path = Path::new(artifact);
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
        match std::fs::symlink_metadata(&source) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            result => {
                result.map_err(file_error)?;
            }
        }
        let mut parent = workspace.clone();
        for part in path.parent().into_iter().flat_map(Path::components) {
            parent.push(part);
            if std::fs::symlink_metadata(&parent)
                .map_err(file_error)?
                .file_type()
                .is_symlink()
            {
                return Err(Error::new(
                    codes::TEST_004,
                    format!("artifact parent {} is a symlink", parent.display()),
                ));
            }
        }
        let destination = results.join("artifacts");
        artifact_directory(&destination)?;
        copy_artifact(&workspace, &source, &destination)?;
    }
    Ok(())
}

// Artifact trees may contain test-generated links, unlike input fixtures.
// Never follow them while copying, and preserve only links that remain inside
// the copied workspace namespace. Missing targets are useful failure evidence.
fn copy_artifact(workspace: &Path, source: &Path, destination: &Path) -> Result<()> {
    let relative = source
        .strip_prefix(workspace)
        .expect("artifact in workspace");
    let target = destination.join(relative);
    let kind = std::fs::symlink_metadata(source)
        .map_err(file_error)?
        .file_type();
    let mut parent = destination.to_path_buf();
    for part in relative.parent().into_iter().flat_map(Path::components) {
        parent.push(part);
        artifact_directory(&parent)?;
    }
    if kind.is_dir() {
        artifact_directory(&target)?;
        let mut entries = std::fs::read_dir(source)
            .map_err(file_error)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(file_error)?;
        entries.sort();
        for entry in entries {
            copy_artifact(workspace, &entry, destination)?;
        }
    } else if kind.is_symlink() {
        let link = std::fs::read_link(source).map_err(file_error)?;
        let mut resolved = relative.parent().unwrap_or(Path::new("")).to_path_buf();
        for part in link.components() {
            match part {
                std::path::Component::Normal(part) => resolved.push(part),
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir if resolved.pop() => {}
                _ => {
                    return Err(Error::new(
                        codes::TEST_004,
                        format!(
                            "artifact symlink {} escapes the workspace: {}",
                            source.display(),
                            link.display()
                        ),
                    ))
                }
            }
        }
        match std::fs::canonicalize(source) {
            Ok(path) if !path.starts_with(workspace) => {
                return Err(Error::new(
                    codes::TEST_004,
                    format!(
                        "artifact symlink {} resolves outside the workspace",
                        source.display()
                    ),
                ))
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(file_error(error))
            }
            _ => {}
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&target) {
            if metadata.file_type().is_symlink()
                && std::fs::read_link(&target).map_err(file_error)? == link
            {
                return Ok(());
            }
            return Err(Error::new(
                codes::TEST_004,
                format!("artifact destination {} already exists", target.display()),
            ));
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(link, target).map_err(file_error)?;
        #[cfg(not(unix))]
        return Err(Error::new(
            codes::TEST_004,
            "artifact symlinks require a Unix host",
        ));
    } else if kind.is_file() {
        if std::fs::symlink_metadata(&target).is_ok_and(|metadata| !metadata.is_file()) {
            return Err(Error::new(
                codes::TEST_004,
                format!(
                    "artifact destination {} is not a regular file",
                    target.display()
                ),
            ));
        }
        std::fs::copy(source, target).map_err(file_error)?;
    } else {
        return Err(Error::new(
            codes::TEST_004,
            format!(
                "artifact {} is not a regular file, directory, or contained relative symlink",
                source.display()
            ),
        ));
    }
    Ok(())
}

fn artifact_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(Error::new(
            codes::TEST_004,
            format!(
                "artifact destination {} is not a real directory",
                path.display()
            ),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path).map_err(file_error)
        }
        Err(error) => Err(file_error(error)),
    }
}

fn finish_test(result: Result<()>, artifacts: Result<()>) -> Result<()> {
    match (result, artifacts) {
        (Err(mut failure), Err(artifact_failure)) => {
            failure.message.push_str(&format!(
                "; artifact collection also failed: {artifact_failure}"
            ));
            Err(failure)
        }
        (Err(failure), _) | (_, Err(failure)) => Err(failure),
        _ => Ok(()),
    }
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
            timeout_seconds: None,
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

#[cfg(test)]
mod transaction_tests {
    use super::*;

    #[test]
    fn private_roots_do_not_share_writable_files() {
        if !std::process::Command::new("unshare")
            .args(["--user", "--map-root-user", "--", "true"])
            .status()
            .is_ok_and(|status| status.success())
        {
            eprintln!("user namespaces are unavailable; skip the root-copy test");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("copy");
        std::fs::create_dir_all(source.join("etc")).unwrap();
        std::fs::write(source.join("etc/probe.conf"), "original").unwrap();
        copy_transaction_root(&source, &destination).unwrap();
        std::fs::write(destination.join("etc/probe.conf"), "changed").unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("etc/probe.conf")).unwrap(),
            "original"
        );
        assert!(copy_transaction_root(Path::new("/"), &directory.path().join("host")).is_err());
        assert!(copy_transaction_root(&source, &source.join("recursive")).is_err());
    }
}

#[cfg(all(test, unix))]
mod artifact_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn test(artifact: &str) -> rpmx_language::PlannedTest {
        rpmx_language::PlannedTest {
            name: "evidence".into(),
            cell_id: String::new(),
            files: None,
            artifacts: vec![artifact.into()],
            skip: None,
            output: None,
            stage: "build".into(),
            argv: None,
            pipeline: vec![],
            packages: vec![],
            environment: BTreeMap::new(),
        }
    }

    #[test]
    fn preserves_openssl_hashlinks_empty_directories_and_dangling_evidence() {
        let work = tempfile::tempdir().unwrap();
        let results = tempfile::tempdir().unwrap();
        let directory = work.path().join("test-runs/test_x509_store");
        std::fs::create_dir_all(directory.join("empty")).unwrap();
        std::fs::write(directory.join("root-cert.pem"), "candidate certificate").unwrap();
        symlink("root-cert.pem", directory.join("8489a545.0")).unwrap();
        symlink("missing.log", directory.join("missing-link")).unwrap();
        symlink("empty", directory.join("directory-link")).unwrap();
        // Input fixtures stay strict; generated output artifacts have a different contract.
        assert!(rpmx_language::test_files::entries(work.path(), "test-runs").is_err());
        collect_artifacts(&test("test-runs"), work.path(), results.path()).unwrap();
        // Overlapping/repeated artifact declarations must not follow or destroy links.
        collect_artifacts(
            &test("test-runs/test_x509_store"),
            work.path(),
            results.path(),
        )
        .unwrap();
        let copied = results.path().join("artifacts/test-runs/test_x509_store");
        assert_eq!(
            std::fs::read_link(copied.join("8489a545.0")).unwrap(),
            Path::new("root-cert.pem")
        );
        assert_eq!(
            std::fs::read(copied.join("8489a545.0")).unwrap(),
            b"candidate certificate"
        );
        assert!(std::fs::symlink_metadata(copied.join("missing-link"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(copied.join("empty").is_dir());
        assert!(std::fs::symlink_metadata(copied.join("directory-link"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn rejects_absolute_escaping_and_indirect_external_links() {
        for link in ["/etc/passwd", "../../outside", "alias/passwd"] {
            let work = tempfile::tempdir().unwrap();
            let results = tempfile::tempdir().unwrap();
            std::fs::create_dir(work.path().join("logs")).unwrap();
            if link == "alias/passwd" {
                symlink("/etc", work.path().join("logs/alias")).unwrap();
            }
            symlink(link, work.path().join("logs/result")).unwrap();
            let error =
                collect_artifacts(&test("logs/result"), work.path(), results.path()).unwrap_err();
            assert_eq!(error.code, codes::TEST_004);
            assert!(
                error.message.contains("outside") || error.message.contains("escapes"),
                "{error}"
            );
            assert!(!results.path().join("artifacts/logs/result").exists());
        }
    }

    #[test]
    fn rejects_symlink_artifact_parents_and_destination_parents() {
        let work = tempfile::tempdir().unwrap();
        let results = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::create_dir(work.path().join("logs")).unwrap();
        std::fs::write(work.path().join("logs/result"), "test").unwrap();
        symlink("logs", work.path().join("alias")).unwrap();
        assert!(collect_artifacts(&test("alias/result"), work.path(), results.path()).is_err());
        std::fs::create_dir(results.path().join("artifacts")).unwrap();
        symlink(external.path(), results.path().join("artifacts/logs")).unwrap();
        assert!(collect_artifacts(&test("logs/result"), work.path(), results.path()).is_err());
        assert!(!external.path().join("result").exists());
    }

    #[test]
    fn rejects_destination_file_symlinks_and_special_artifacts() {
        let work = tempfile::tempdir().unwrap();
        let results = tempfile::tempdir().unwrap();
        let external = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(external.path(), "unchanged").unwrap();
        std::fs::write(work.path().join("result"), "overwrite").unwrap();
        std::fs::create_dir(results.path().join("artifacts")).unwrap();
        symlink(external.path(), results.path().join("artifacts/result")).unwrap();
        assert!(collect_artifacts(&test("result"), work.path(), results.path()).is_err());
        assert_eq!(std::fs::read(external.path()).unwrap(), b"unchanged");
        let _socket = std::os::unix::net::UnixListener::bind(work.path().join("socket")).unwrap();
        let error = collect_artifacts(&test("socket"), work.path(), results.path()).unwrap_err();
        assert!(error.message.contains("not a regular file"), "{error}");
    }

    #[test]
    fn retains_original_failure_when_artifact_collection_also_fails() {
        let error = finish_test(
            Err(Error::new(codes::TEST_003, "OCSP assertion failed, exit 1")),
            Err(Error::new(codes::TEST_004, "escaping artifact symlink")),
        )
        .unwrap_err();
        assert_eq!(error.code, codes::TEST_003);
        assert!(error.message.contains("OCSP assertion failed, exit 1"));
        assert!(error.message.contains("artifact collection also failed"));
        assert!(error.message.contains("escaping artifact symlink"));
        assert_eq!(
            finish_test(Ok(()), Err(Error::new(codes::TEST_004, "bad evidence")))
                .unwrap_err()
                .code,
            codes::TEST_004
        );
    }

    #[test]
    #[ignore = "requires an existing OpenSSL source workspace with test-runs evidence"]
    fn copies_actual_openssl_test_evidence() {
        let workspace = std::path::PathBuf::from(
            std::env::var_os("RPMX_TEST_OPENSSL_WORKSPACE").expect("RPMX_TEST_OPENSSL_WORKSPACE"),
        );
        let results = tempfile::tempdir().unwrap();
        collect_artifacts(&test("test-runs"), &workspace, results.path()).unwrap();
        let hashlink = results
            .path()
            .join("artifacts/test-runs/test_x509_store/60-test_x509_store/8489a545.0");
        assert!(std::fs::symlink_metadata(&hashlink)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(std::fs::read_to_string(hashlink)
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
    }
}
