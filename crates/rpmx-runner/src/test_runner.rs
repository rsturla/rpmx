use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::Path;

use crate::sandbox::{Sandbox, SandboxConfig};
use rpmx_base::error::{codes, Error, Result};
use rpmx_language::plan::PlannedStep;

/// The application supplies a verified environment. The executor replaces
/// sandbox.workspace and sandbox.stage with its private case paths. No caller
/// needs to instantiate a dummy Sandbox or create placeholder directories.
pub struct TestEnvironment<'a> {
    pub sandbox: SandboxConfig<'a>,
    pub workspace: WorkspaceSource<'a>,
    pub fixture_root: &'a Path,
    pub payload_root: &'a Path,
    pub rpms: &'a [std::path::PathBuf],
}

#[derive(Clone, Copy)]
pub enum WorkspaceSource<'a> {
    Empty,
    Copy(&'a Path),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
}

pub struct TestExecutionReport {
    pub target_arch: rpmx_base::Arch,
    pub execution_arch: rpmx_base::Arch,
    pub name: String,
    pub status: TestStatus,
    pub reason: Option<String>,
    pub duration: std::time::Duration,
    pub evidence: std::path::PathBuf,
    pub primary: Option<Error>,
    pub secondary: Vec<Error>,
}
impl TestExecutionReport {
    fn failure(&mut self, error: Error) {
        self.status = TestStatus::Failed;
        if self.primary.is_none() {
            self.primary = Some(error);
        } else {
            self.secondary.push(error);
        }
    }
}

/// One execution path for build and installed tests. Even preparation failures
/// return a receipt, and collection failures never replace the command error.
pub fn execute_test(
    test: &rpmx_language::PlannedTest,
    environment: TestEnvironment<'_>,
    evidence: &Path,
) -> TestExecutionReport {
    let started = std::time::Instant::now();
    let mut report = TestExecutionReport {
        target_arch: environment.sandbox.target,
        execution_arch: environment.sandbox.exec,
        name: test.qualified_name(),
        status: TestStatus::Passed,
        reason: None,
        duration: std::time::Duration::ZERO,
        evidence: result_dir(evidence, test),
        primary: None,
        secondary: Vec::new(),
    };
    if let Some(reason) = &test.skip {
        report.status = TestStatus::Skipped;
        report.reason = Some(reason.clone());
        report.duration = started.elapsed();
        return report;
    }
    if let Err(error) = execute_case(test, environment, started, &mut report) {
        report.failure(error);
    }
    report.duration = started.elapsed();
    report
}

fn execute_case(
    test: &rpmx_language::PlannedTest,
    environment: TestEnvironment<'_>,
    started: std::time::Instant,
    report: &mut TestExecutionReport,
) -> Result<()> {
    std::fs::create_dir_all(&report.evidence).map_err(file_error)?;
    let work = tempfile::tempdir().map_err(file_error)?;
    let workspace = work.path().join("build");
    let stage = environment.payload_root;
    std::fs::create_dir_all(&workspace).map_err(file_error)?;
    if let WorkspaceSource::Copy(source) = environment.workspace {
        copy_workspace(source, &workspace)?;
    }
    let transaction = test.mode == rpmx_language::TestMode::Transaction;
    let private_root = work.path().join("root");
    let _cleanup = TransactionRootCleanup(private_root.clone());
    if transaction {
        let source = environment.sandbox.root.ok_or_else(|| {
            Error::new(
                codes::TEST_004,
                "transaction tests require a verified package root",
            )
        })?;
        copy_transaction_root(source, &private_root)?;
    }
    let mut test = test.clone();
    let fixture_directory = work.path().join("fixtures");
    std::fs::create_dir_all(&fixture_directory).map_err(file_error)?;
    let mut mounts = vec![crate::runner::ReadOnlyMount {
        source: environment.payload_root,
        target: "/tmp/rpmx-inputs/package",
    }];
    test.environment.insert(
        "TEST_PACKAGE_ROOT".into(),
        "/tmp/rpmx-inputs/package".into(),
    );
    if let Some(files) = &test.files {
        snapshot_files(environment.fixture_root, files, &fixture_directory)?;
        mounts.push(crate::runner::ReadOnlyMount {
            source: &fixture_directory,
            target: "/tmp/rpmx-inputs/files",
        });
        test.environment
            .insert("TEST_FILES".into(), "/tmp/rpmx-inputs/files".into());
    }
    let rpm_directory = work.path().join("rpms");
    if !environment.rpms.is_empty() {
        std::fs::create_dir_all(&rpm_directory).map_err(file_error)?;
        for rpm in environment.rpms {
            let name = rpm
                .file_name()
                .ok_or_else(|| Error::new(codes::TEST_004, "test RPM has no filename"))?;
            let destination = rpm_directory.join(name);
            if destination.exists() {
                return Err(Error::new(
                    codes::TEST_004,
                    format!("duplicate test RPM filename {}", destination.display()),
                ));
            }
            std::fs::copy(rpm, destination).map_err(file_error)?;
        }
        mounts.push(crate::runner::ReadOnlyMount {
            source: &rpm_directory,
            target: "/tmp/rpmx-inputs/rpms",
        });
        test.environment
            .insert("TEST_RPMS".into(), "/tmp/rpmx-inputs/rpms".into());
    }
    // Step fixtures have their own directory and remain read-only too.
    let mut step_directories = Vec::new();
    for (index, step) in test.pipeline.iter_mut().enumerate() {
        if let Some(files) = &step.files {
            let directory = work.path().join(format!("step-{index}-files"));
            std::fs::create_dir_all(&directory).map_err(file_error)?;
            snapshot_files(environment.fixture_root, files, &directory)?;
            let guest = format!("/tmp/rpmx-inputs/step-{index}");
            step.env.insert("RPMX_FILES".into(), guest.clone());
            step_directories.push((directory, guest));
        }
    }
    for (source, target) in &step_directories {
        mounts.push(crate::runner::ReadOnlyMount { source, target });
    }
    let deadline = started
        + std::time::Duration::from_secs(
            test.timeout_seconds
                .unwrap_or(environment.sandbox.timeout)
                .get(),
        );
    let policy = crate::runner::ExecutionPolicy {
        deadline: Some(deadline),
        network: test.network,
        readonly_stage: true,
        readonly_mounts: &mounts,
    };
    let mut config = environment.sandbox;
    config.workspace = &workspace;
    config.stage = stage;
    config.writable_root = transaction;
    if transaction {
        config.root = Some(&private_root);
    }
    let sandbox = Sandbox::with_policy(config, policy)?;
    let command = run_one_test(&sandbox, &test, &report.evidence, deadline);
    if let Err(error) = command {
        report.failure(error);
    }
    // Finish the backend before reading any test output; background children
    // must not keep writing while evidence is being copied.
    if let Err(error) = sandbox.finish() {
        report.failure(error);
    }
    let mut artifacts = test.clone();
    artifacts
        .artifacts
        .extend(test.pipeline.iter().flat_map(|step| step.artifacts.clone()));
    if let Err(error) = collect_artifacts(&artifacts, &workspace, &report.evidence) {
        report.failure(error);
    }
    if std::time::Instant::now() > deadline && report.status == TestStatus::Passed {
        report.failure(Error::new(codes::TEST_003, "test case exceeded its wall-clock deadline while completing cleanup or collecting evidence"));
    }
    if rpmx_base::output::keep_workdir() {
        rpmx_base::output::wrote(&work.keep());
    }
    Ok(())
}

/// Copies preserve links within the new tree, but never share writable inode
/// identity with the source. Reflinks use copy-on-write rather than hardlinks.
pub fn copy_workspace(source: &Path, destination: &Path) -> Result<()> {
    let source = std::fs::canonicalize(source).map_err(file_error)?;
    if destination.starts_with(&source) {
        return Err(Error::new(
            codes::TEST_004,
            "workspace copy cannot be inside its source",
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
                "cannot copy test workspace: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    Ok(())
}

fn snapshot_files(root: &Path, directory: &str, destination: &Path) -> Result<()> {
    let source = root.join(directory);
    for path in rpmx_language::test_files::entries(root, directory)? {
        let target = destination.join(path.strip_prefix(&source).expect("validated fixture path"));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(file_error)?;
        }
        std::fs::copy(path, target).map_err(file_error)?;
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

#[cfg(test)]
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
    deadline: std::time::Instant,
) -> Result<()> {
    let directory = results.to_path_buf();
    std::fs::create_dir_all(&directory).map_err(file_error)?;
    let logs = crate::runner::OutputLogs {
        stdout: std::fs::File::create(directory.join("stdout.log")).map_err(file_error)?,
        stderr: std::fs::File::create(directory.join("stderr.log")).map_err(file_error)?,
    };
    let _phase = rpmx_base::output::enter_phase("test");
    rpmx_base::output::phase("test", format!("{} ({})", test.qualified_name(), test.mode));
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
            files: None,
            artifacts: Vec::new(),
        };
        position += 1;
        let step = within_deadline(step, deadline)?;
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
        let step = within_deadline(step, deadline)?;
        sandbox
            .run_recorded(&step, position, total, &logs)
            .map_err(test_failure)?;
    }
    if std::time::Instant::now() > deadline {
        return Err(Error::new(
            codes::TEST_003,
            "test case exceeded its wall-clock deadline",
        ));
    }
    Ok(())
}

fn within_deadline(mut step: PlannedStep, deadline: std::time::Instant) -> Result<PlannedStep> {
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return Err(Error::new(
            codes::TEST_003,
            "test case deadline expired before the next step",
        ));
    }
    // Round up subsecond remainder; backend deadlines are whole seconds. The
    // wall clock deadline is checked again after every command.
    let seconds = remaining
        .as_secs()
        .saturating_add(u64::from(remaining.subsec_nanos() != 0))
        .max(1);
    let seconds = step
        .timeout_seconds
        .map(|timeout| timeout.get().min(seconds))
        .unwrap_or(seconds);
    step.timeout_seconds = NonZeroU64::new(seconds);
    Ok(step)
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
            mode: rpmx_language::TestMode::Installed,
            network: false,
            timeout_seconds: None,
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

#[cfg(all(test, unix))]
mod execution_tests {
    use super::*;
    use crate::runner::{RunnerAssets, RunnerKind};
    use rpmx_base::Arch;
    use std::num::NonZeroU32;
    use std::path::Path;

    fn available() -> bool {
        std::process::Command::new("bwrap")
            .args(["--unshare-all", "--ro-bind", "/", "/", "true"])
            .status()
            .is_ok_and(|status| status.success())
    }
    fn step(script: &str) -> PlannedStep {
        PlannedStep {
            label: "probe".into(),
            uses: "probe".into(),
            command: Some("bash".into()),
            args: vec!["-euc".into(), script.into()],
            cwd: None,
            timeout_seconds: None,
            env: BTreeMap::new(),
            needs: vec![],
            needs_packages: vec![],
            needs_sysroot: vec![],
            tool_digest: None,
            step_digest: None,
            builtin: false,
            inputs: BTreeMap::new(),
            input: None,
            input_file: None,
            files: None,
            artifacts: vec![],
        }
    }
    fn case(script: &str) -> rpmx_language::PlannedTest {
        rpmx_language::PlannedTest {
            name: "probe".into(),
            cell_id: "native".into(),
            files: None,
            artifacts: vec![],
            skip: None,
            output: None,
            mode: rpmx_language::TestMode::Installed,
            network: false,
            timeout_seconds: None,
            argv: None,
            pipeline: vec![step(script)],
            packages: vec![],
            environment: BTreeMap::new(),
        }
    }
    fn config<'a>(workspace: &'a Path, stage: &'a Path) -> SandboxConfig<'a> {
        let arch = Arch::current().unwrap();
        SandboxConfig {
            writable_root: false,
            runner: RunnerKind::Bwrap,
            host: arch,
            exec: arch,
            target: arch,
            sysroot: None,
            cross: None,
            workspace,
            stage,
            root: None,
            assets: RunnerAssets::default(),
            image: None,
            prefix: "/usr",
            libdir: "/usr/lib64",
            source_date_epoch: 0,
            jobs: NonZeroU32::new(1).unwrap(),
            timeout: NonZeroU64::new(10).unwrap(),
        }
    }
    fn execute(
        test: &rpmx_language::PlannedTest,
        root: &Path,
        workspace: WorkspaceSource<'_>,
        evidence: &Path,
    ) -> TestExecutionReport {
        let payload = root.join("payload");
        std::fs::create_dir_all(&payload).unwrap();
        execute_test(
            test,
            TestEnvironment {
                sandbox: config(root, &payload),
                workspace,
                fixture_root: root,
                payload_root: &payload,
                rpms: &[],
            },
            evidence,
        )
    }
    #[test]
    #[ignore = "requires rootless Podman and overlay storage"]
    fn podman_uses_private_root_and_enforces_case_mounts_and_identity() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        for command in [
            "/usr/bin/bash",
            "/usr/bin/sh",
            "/usr/bin/sleep",
            "/usr/bin/id",
            "/usr/bin/cat",
        ] {
            let output = std::process::Command::new("ldd")
                .arg(command)
                .output()
                .unwrap();
            assert!(output.status.success());
            let libraries = String::from_utf8(output.stdout).unwrap();
            for source in std::iter::once(command).chain(
                libraries
                    .split_whitespace()
                    .filter(|word| word.starts_with('/')),
            ) {
                let target = root.join(source.trim_start_matches('/'));
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::copy(source, target).unwrap();
            }
        }
        for path in ["tmp", "dev", "proc", "sys", "etc"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        let payload = directory.path().join("payload");
        std::fs::create_dir(&payload).unwrap();
        std::fs::write(payload.join("value"), "production").unwrap();
        std::os::unix::fs::symlink("usr/bin", root.join("bin")).unwrap();
        let before = rpmx_store::tree::digest_root(&root).unwrap();
        let mut test = case(
            r#"
set -x
test "$(id -u)" = 65534
test "$(id -g)" = 65534
test "$(cat "$TEST_PACKAGE_ROOT/value")" = production
if printf changed >"$TEST_PACKAGE_ROOT/value"; then exit 71; fi
if printf changed >"$STAGE/value"; then exit 72; fi
if printf changed >/etc/host-mutation; then exit 73; fi
printf 'private workspace' >result
"#,
        );
        test.artifacts = vec!["result".into()];
        let mut sandbox = config(directory.path(), &payload);
        sandbox.runner = RunnerKind::Podman;
        sandbox.root = Some(&root);
        let result = execute_test(
            &test,
            TestEnvironment {
                sandbox,
                workspace: WorkspaceSource::Empty,
                fixture_root: directory.path(),
                payload_root: &payload,
                rpms: &[],
            },
            &directory.path().join("evidence"),
        );
        assert_eq!(
            result.status,
            TestStatus::Passed,
            "{:?} {:?}",
            result.primary,
            result.secondary
        );
        assert_eq!(
            std::fs::read_to_string(payload.join("value")).unwrap(),
            "production"
        );
        assert_eq!(
            rpmx_store::tree::digest_root(&root).unwrap(),
            before,
            "Podman must not modify the verified lower root"
        );
        assert!(result.evidence.join("artifacts/result").is_file());

        // A command may leave a daemon behind. Finishing the build session
        // must terminate it before callers seal or export the workspace.
        let direct_workspace = directory.path().join("build-session");
        std::fs::create_dir(&direct_workspace).unwrap();
        let mut direct_config = config(&direct_workspace, &payload);
        direct_config.runner = RunnerKind::Podman;
        direct_config.root = Some(&root);
        let sandbox = Sandbox::new(direct_config).unwrap();
        sandbox
            .run(&step(
                "(while :; do printf x >>late-writer; sleep 0.05; done) >/dev/null 2>&1 &",
            ))
            .unwrap();
        sandbox.finish().unwrap();
        let after_finish = std::fs::read(direct_workspace.join("late-writer")).unwrap_or_default();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(
            std::fs::read(direct_workspace.join("late-writer")).unwrap_or_default(),
            after_finish,
            "a container writer survived session completion"
        );
    }
    #[test]
    fn copying_protected_stage_files_preserves_bytes_and_modes_without_changing_the_source() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let copy = directory.path().join("copy");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("protected"), "protected bytes").unwrap();
        std::fs::set_permissions(
            source.join("protected"),
            std::fs::Permissions::from_mode(0o0),
        )
        .unwrap();
        copy_workspace(&source, &copy).unwrap();
        for root in [&source, &copy] {
            assert_eq!(
                std::fs::metadata(root.join("protected"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o0
            );
            let output = std::process::Command::new("unshare")
                .args(["--user", "--map-root-user", "--", "cat"])
                .arg(root.join("protected"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, b"protected bytes");
        }
        assert_ne!(
            std::os::unix::fs::MetadataExt::ino(
                &std::fs::metadata(source.join("protected")).unwrap()
            ),
            std::os::unix::fs::MetadataExt::ino(
                &std::fs::metadata(copy.join("protected")).unwrap()
            )
        );
    }
    #[test]
    fn immutable_inputs_and_workspace_copies_prevent_alias_writes() {
        if !available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let fixtures = root.path().join("fixtures");
        let payload = root.path().join("payload");
        let source = root.path().join("source");
        for directory in [&fixtures, &payload, &source] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(fixtures.join("input"), "fixture\n").unwrap();
        std::fs::write(payload.join("output"), "production\n").unwrap();
        std::fs::hard_link(payload.join("output"), source.join("compiled")).unwrap();
        let mut test = case(
            r#"
set -x
test "$(cat "$TEST_FILES/input")" = fixture
if printf changed >"$TEST_FILES/input"; then exit 71; fi
if printf changed >"$STAGE/output"; then exit 72; fi
if printf changed >"$TEST_PACKAGE_ROOT/output"; then exit 73; fi
printf 'private workspace\n' >compiled
printf 'retained evidence\n' >result.log
"#,
        );
        test.files = Some("fixtures".into());
        test.artifacts = vec!["result.log".into()];
        let result = execute(
            &test,
            root.path(),
            WorkspaceSource::Copy(&source),
            &root.path().join("evidence"),
        );
        assert_eq!(result.status, TestStatus::Passed, "{:?}", result.primary);
        assert_eq!(
            std::fs::read_to_string(payload.join("output")).unwrap(),
            "production\n"
        );
        assert_eq!(
            std::fs::read_to_string(source.join("compiled")).unwrap(),
            "production\n"
        );
        assert_eq!(
            std::fs::read_to_string(fixtures.join("input")).unwrap(),
            "fixture\n"
        );
        assert!(result.evidence.join("artifacts/result.log").is_file());
    }
    #[test]
    fn one_deadline_covers_all_steps() {
        if !available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let mut test = case("sleep 0.7");
        test.pipeline
            .push(step("sleep 0.7; printf should-not-complete"));
        test.timeout_seconds = NonZeroU64::new(1);
        let result = execute(
            &test,
            root.path(),
            WorkspaceSource::Empty,
            &root.path().join("evidence"),
        );
        assert_eq!(result.status, TestStatus::Failed);
        assert_eq!(result.primary.unwrap().code, codes::TEST_003);
        assert!(
            result.duration < std::time::Duration::from_millis(1350),
            "deadline reset between steps: {:?}",
            result.duration
        );
        assert!(!std::fs::read_to_string(result.evidence.join("stdout.log"))
            .unwrap()
            .contains("should-not-complete"));
    }
    #[test]
    fn command_and_artifact_failures_both_survive() {
        if !available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let mut test = case("printf 'original failure evidence\\n'; ln -s /etc/passwd bad; exit 7");
        test.artifacts = vec!["bad".into()];
        let result = execute(
            &test,
            root.path(),
            WorkspaceSource::Empty,
            &root.path().join("evidence"),
        );
        assert_eq!(result.status, TestStatus::Failed);
        assert_eq!(result.primary.unwrap().code, codes::TEST_003);
        assert!(result
            .secondary
            .iter()
            .any(|error| error.code == codes::TEST_004));
        assert!(std::fs::read_to_string(result.evidence.join("stdout.log"))
            .unwrap()
            .contains("original failure evidence"));
    }
    #[test]
    fn isolated_network_supports_local_services_without_a_host_route() {
        if !available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let test = case(
            r#"python3 - <<'PYTHON'
import socket
import threading
from pathlib import Path

# A local server and its client belong in the same command: native command
# completion reaps background children rather than leaking service processes.
with socket.socket() as server:
    server.bind(("127.0.0.1", 0))
    server.listen(1)
    def respond():
        with server.accept()[0] as connection:
            assert connection.recv(16) == b"request"
            connection.sendall(b"response")
    worker = threading.Thread(target=respond, daemon=True)
    worker.start()
    with socket.create_connection(server.getsockname(), timeout=2) as client:
        client.sendall(b"request")
        assert client.recv(16) == b"response"
    worker.join(timeout=2)
    assert not worker.is_alive(), "loopback service did not complete"
routes = Path("/proc/net/route").read_text().splitlines()[1:]
assert not any(line.split()[1] == "00000000" for line in routes), routes
print("isolated-loopback-ok")
PYTHON"#,
        );
        let result = execute(
            &test,
            root.path(),
            WorkspaceSource::Empty,
            &root.path().join("evidence"),
        );
        assert_eq!(result.status, TestStatus::Passed, "{:?}", result.primary);
        assert!(std::fs::read_to_string(result.evidence.join("stdout.log"))
            .unwrap()
            .contains("isolated-loopback-ok"));
    }
    #[test]
    fn network_is_disabled_unless_this_case_requests_it() {
        if !available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let host = std::fs::read_link("/proc/self/ns/net").unwrap();
        let mut test = case("readlink /proc/self/ns/net >network");
        test.artifacts = vec!["network".into()];
        let result = execute(
            &test,
            root.path(),
            WorkspaceSource::Empty,
            &root.path().join("offline"),
        );
        assert_eq!(result.status, TestStatus::Passed, "{:?}", result.primary);
        let network = std::fs::read_to_string(result.evidence.join("artifacts/network")).unwrap();
        assert_ne!(network.trim(), host.to_str().unwrap());
        test.network = true;
        let result = execute(
            &test,
            root.path(),
            WorkspaceSource::Empty,
            &root.path().join("online"),
        );
        assert_eq!(result.status, TestStatus::Passed, "{:?}", result.primary);
        let network = std::fs::read_to_string(result.evidence.join("artifacts/network")).unwrap();
        assert_eq!(network.trim(), host.to_str().unwrap());
    }
}
