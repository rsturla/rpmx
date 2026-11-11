use rpmx_application::{
    AcceptInput, AcceptanceEnvironment, AcceptanceState, Application, ApplicationConfig,
    ApplicationRequest, CaseStatus, Operation, RpmInputs, RunOutcome,
};
use std::path::{Path, PathBuf};

fn recipe(directory: &Path, tests: &str) -> PathBuf {
    let path = directory.join("package.yaml");
    std::fs::write(&path, format!("schema: 2\npackage:\n  name: fixture\n  version: '1'\n  release: 1\n  summary: test fixture\n  license: MIT\noutputs:\n  - name: fixture\n    arch: noarch\n    metapackage: true\n    tests:\n{tests}\n")).unwrap();
    path
}

fn rpm(directory: &Path) -> PathBuf {
    let path = directory.join("fixture-1-1.noarch.rpm");
    let mut builder = rpm::PackageBuilder::new("fixture", "1", "MIT", "noarch", "test fixture");
    builder.release("1");
    builder.build().unwrap().write_file(&path).unwrap();
    path
}

fn run(
    directory: &Path,
    recipe: PathBuf,
    subjects: RpmInputs,
    selection: Vec<String>,
) -> rpmx_application::RunReport {
    let app = Application::new(ApplicationConfig {
        cache: Some(directory.join("cache")),
        offline: true,
        ..ApplicationConfig::default()
    });
    app.execute(ApplicationRequest {
        operation: Operation::Accept(AcceptInput {
            recipe,
            subjects,
            arch: None,
            selection,
            environment: AcceptanceEnvironment::ProjectDefault,
            save_environment: None,
        }),
        output: directory.join("results"),
    })
    .unwrap()
}

#[test]
fn unavailable_rpm_inputs_are_failed_runs_with_json_and_junit() {
    let directory = tempfile::tempdir().unwrap();
    let report = run(
        directory.path(),
        directory.path().join("unused.yaml"),
        RpmInputs::Directory(directory.path().join("missing")),
        vec![],
    );
    assert_eq!(report.outcome, RunOutcome::Failed);
    let saved: rpmx_application::RunReport = serde_json::from_slice(
        &std::fs::read(directory.path().join("results/run-report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved.outcome, RunOutcome::Failed);
    let junit = std::fs::read_to_string(directory.path().join("results/run-report.xml")).unwrap();
    assert!(junit.contains("name=\"preparation\""));
    assert!(junit.contains("failures=\"0\" errors=\"1\""));
    assert!(junit.contains("missing"));
}

#[test]
fn offline_network_tests_are_not_run_and_cannot_pass_the_run() {
    let directory = tempfile::tempdir().unwrap();
    let path = recipe(
        directory.path(),
        "      - name: live-service\n        network: true\n        script: 'exit 99'",
    );
    let subject = rpm(directory.path());
    let report = run(
        directory.path(),
        path,
        RpmInputs::Files(vec![subject]),
        vec![],
    );
    assert_eq!(report.outcome, RunOutcome::Incomplete);
    assert_eq!(report.acceptance, AcceptanceState::Completed);
    assert_eq!(report.tests.len(), 1);
    assert_eq!(report.tests[0].status, CaseStatus::NotRun);
    assert!(report.tests[0]
        .reason
        .as_deref()
        .unwrap()
        .contains("offline"));
    assert!(report.environments.is_empty());
    assert!(!report.success());
    assert!(report.require_success().is_err());
}

#[test]
fn skipped_cases_preserve_the_reason_without_acquiring_a_root() {
    let directory = tempfile::tempdir().unwrap();
    let path = recipe(directory.path(), "      - name: unavailable-platform\n        skip: requires a hardware fixture\n        script: 'exit 99'");
    let subject = rpm(directory.path());
    let report = run(
        directory.path(),
        path,
        RpmInputs::Files(vec![subject]),
        vec![],
    );
    assert_eq!(report.tests[0].status, CaseStatus::Skipped);
    assert_eq!(
        report.tests[0].reason.as_deref(),
        Some("requires a hardware fixture")
    );
    assert_eq!(report.outcome, RunOutcome::Incomplete);
    assert!(report.environments.is_empty());
}

#[test]
fn stale_same_name_rpms_fail_before_any_environment_is_prepared() {
    let directory = tempfile::tempdir().unwrap();
    let path = recipe(
        directory.path(),
        "      - name: behavior\n        script: 'exit 0'",
    );
    let bytes = std::fs::read_to_string(&path)
        .unwrap()
        .replace("version: '1'", "version: '2'");
    std::fs::write(&path, bytes).unwrap();
    let subject = rpm(directory.path());
    let report = run(
        directory.path(),
        path,
        RpmInputs::Files(vec![subject]),
        vec![],
    );
    assert_eq!(report.outcome, RunOutcome::Failed);
    assert!(report
        .primary
        .as_ref()
        .unwrap()
        .message
        .contains("does not match planned output"));
    assert!(report.environments.is_empty());
}

#[test]
fn unknown_selection_is_reported_before_environment_acquisition() {
    let directory = tempfile::tempdir().unwrap();
    let path = recipe(
        directory.path(),
        "      - name: behavior\n        script: 'exit 0'",
    );
    let subject = rpm(directory.path());
    let report = run(
        directory.path(),
        path,
        RpmInputs::Files(vec![subject]),
        vec!["typo".into()],
    );
    assert_eq!(report.outcome, RunOutcome::Failed);
    assert!(report
        .primary
        .as_ref()
        .unwrap()
        .message
        .contains("no test named 'typo'"));
    assert!(report.environments.is_empty());
}

/// This runs DNF's real solver and installer, then native isolated commands.
/// Every repository payload is generated locally; no package download is used.
#[test]
#[ignore = "requires cached Fedora 44 builder image, Podman and configured bubblewrap namespaces"]
fn package_tests_resolve_only_required_outputs_and_replay_exact_environments() {
    let directory = tempfile::tempdir().unwrap();
    let subject = rpm(directory.path());
    let runtime = local_runtime(directory.path());
    let extra = directory.path().join("unrelated.rpm");
    let mut builder = rpm::PackageBuilder::new(
        "unrelated-compiler",
        "1",
        "MIT",
        "noarch",
        "must not enter test roots",
    );
    builder
        .with_file_contents(b"unrelated", rpm::FileOptions::new("/unrelated-compiler"))
        .unwrap();
    builder.build().unwrap().write_file(&extra).unwrap();
    std::fs::create_dir(directory.path().join("tests")).unwrap();
    std::fs::write(directory.path().join("tests/input"), "fixture\n").unwrap();
    lifecycle_package(&directory.path().join("tests/previous.rpm"), "1");
    lifecycle_package(&directory.path().join("tests/current.rpm"), "2");
    let path = recipe(
        directory.path(),
        r#"      - name: readonly-inputs
        files: tests
        timeout: 20s
        script: |
          read -r value < "$TEST_FILES/input"
          test "$value" = fixture
          test ! -e /unrelated-compiler
          test ! -e /published-only
          test ! -e "$TEST_PACKAGE_ROOT/usr/bin/bash"
          if printf changed > "$TEST_FILES/input"; then exit 1; fi
          if printf changed > "$STAGE/changed"; then exit 1; fi
          printf evidence > receipt
        artifacts: [receipt]
      - name: private-transaction
        mode: transaction
        files: tests
        script: |
          rpm() { command rpm --noplugins --dbpath /usr/lib/sysimage/rpm "$@"; }
          rpm -q fixture
          rpm -i --nosignature "$TEST_FILES/previous.rpm"
          read -r value < /lifecycle-probe.post
          test "$value" = 1
          printf 'user-value\n' > /etc/lifecycle-probe.conf
          rpm -U --nosignature "$TEST_FILES/current.rpm"
          test "$(rpm -q --qf '%{VERSION}' lifecycle-probe)" = 2
          read -r value < /etc/lifecycle-probe.conf
          test "$value" = user-value
          read -r value < /etc/lifecycle-probe.conf.rpmnew
          test "$value" = default-2
          read -r value < /lifecycle-probe.post
          test "$value" = 2
          read -r value < /lifecycle-probe.postun
          test "$value" = 1
          rpm -e lifecycle-probe
          test ! -e /usr/share/lifecycle-probe/version
          read -r value < /lifecycle-probe.postun
          test "$value" = 0
          if rpm -q lifecycle-probe; then exit 1; fi
          printf private > /transaction-sentinel
      - name: fresh-installed-root
        script: |
          test ! -e /transaction-sentinel
          test ! -e /lifecycle-probe.post
          test ! -e /lifecycle-probe.postun"#,
    );
    let profile = directory.path().join("profile.yaml");
    let repository = directory.path().join("repository");
    rpmx_rpm::repo::generate(rpmx_rpm::repo::RepoOptions {
        rpms: directory.path(),
        output: &repository,
        signing_key: None,
        timestamp: 0,
    })
    .unwrap();
    // Published bytes differ at the same NEVRA. Only the caller's candidate
    // may be installed, even though both repositories offer that identity.
    let temporary = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
    let mut published =
        rpm::PackageBuilder::new("fixture", "1", "MIT", "noarch", "published replacement");
    published.release("1");
    published
        .with_file_contents(b"published", rpm::FileOptions::new("/published-only"))
        .unwrap();
    published
        .build()
        .unwrap()
        .write_file(temporary.path())
        .unwrap();
    temporary
        .persist(repository.join("fixture-1-1.noarch.rpm"))
        .unwrap();
    rpmx_rpm::repo::generate(rpmx_rpm::repo::RepoOptions {
        rpms: &repository,
        output: &repository,
        signing_key: None,
        timestamp: 0,
    })
    .unwrap();
    std::fs::write(&profile, format!("document: rpmx-profile\nversion: 1\nreleasever: '44'\narchs: [{}]\nrepositories:\n  - id: local\n    baseurl: file://{}\n", rpmx_base::arch::Arch::current().unwrap(), repository.display())).unwrap();
    let saved = directory.path().join("saved");
    let subjects = RpmInputs::Files(vec![subject, runtime, extra]);
    let config = ApplicationConfig {
        cache: Some(directory.path().join("cache")),
        runner: "bwrap".into(),
        ..ApplicationConfig::default()
    };
    let input = AcceptInput {
        recipe: path,
        subjects,
        arch: None,
        selection: vec![],
        environment: AcceptanceEnvironment::Profile(profile),
        save_environment: Some(saved.clone()),
    };
    let request = |input| ApplicationRequest {
        operation: Operation::Accept(input),
        output: directory.path().join("results"),
    };
    let report = Application::new(config.clone())
        .execute(request(input.clone()))
        .unwrap();
    assert!(
        report.success(),
        "{}",
        serde_json::to_string_pretty(&report).unwrap()
    );
    assert_eq!(report.tests.len(), 3);
    assert_eq!(report.environments.len(), 2);
    let lock = rpmx_root::root::load_lock(&report.environments[0].lock).unwrap();
    assert!(!lock
        .packages
        .values()
        .flatten()
        .any(|package| package.name == "unrelated-compiler"));
    assert_eq!(
        std::fs::read(report.tests[0].evidence.join("artifacts/receipt")).unwrap(),
        b"evidence"
    );
    let mut replay = input;
    replay.environment = AcceptanceEnvironment::Replay(saved.clone());
    replay.save_environment = None;
    let report = Application::new(ApplicationConfig {
        offline: true,
        ..config.clone()
    })
    .execute(request(replay.clone()))
    .unwrap();
    assert!(
        report.success(),
        "{}",
        serde_json::to_string_pretty(&report).unwrap()
    );
    assert_eq!(report.tests.len(), 3);

    // Replay must use the saved tree, even when all packages are cached.
    // Removing one group's tree must fail that group without rebuilding it.
    let group = std::fs::read_dir(&saved)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("root").is_dir())
        .unwrap();
    let missing = group.join("root");
    std::fs::rename(&missing, group.join("removed-root")).unwrap();
    let report = Application::new(ApplicationConfig {
        offline: true,
        ..config
    })
    .execute(request(replay))
    .unwrap();
    assert_eq!(report.outcome, RunOutcome::Failed);
    assert!(report
        .tests
        .iter()
        .any(|case| case.status == CaseStatus::Failed));
    assert!(
        !missing.exists(),
        "replay must not reconstruct a missing root"
    );
}

fn lifecycle_package(path: &Path, version: &str) {
    let mut builder = rpm::PackageBuilder::new(
        "lifecycle-probe",
        version,
        "MIT",
        "noarch",
        "transaction fixture",
    );
    builder.release("1");
    builder
        .with_file_contents(
            format!("default-{version}\n"),
            rpm::FileOptions::new("/etc/lifecycle-probe.conf")
                .mode(0o100644)
                .config()
                .noreplace(),
        )
        .unwrap();
    builder
        .with_file_contents(
            version,
            rpm::FileOptions::new("/usr/share/lifecycle-probe/version").mode(0o100644),
        )
        .unwrap();
    builder.post_install_script(
        rpm::Scriptlet::new("printf '%s\\n' \"$1\" > /lifecycle-probe.post")
            .prog(vec!["/usr/bin/bash"]),
    );
    builder.post_uninstall_script(
        rpm::Scriptlet::new("printf '%s\\n' \"$1\" > /lifecycle-probe.postun")
            .prog(vec!["/usr/bin/bash"]),
    );
    builder.build().unwrap().write_file(path).unwrap();
}

fn local_runtime(directory: &Path) -> PathBuf {
    use std::collections::BTreeSet;
    let path = directory.join("runtime.rpm");
    let arch = rpmx_base::arch::Arch::current().unwrap();
    let mut builder = rpm::PackageBuilder::new(
        "fixture-runtime",
        "1",
        "MIT",
        arch.as_str(),
        "private execution tools",
    );
    builder.provides(rpm::Dependency::any("bash"));
    builder.provides(rpm::Dependency::any("coreutils"));
    builder.provides(rpm::Dependency::any("rpm"));
    let mut files = BTreeSet::new();
    for command in [
        "/usr/bin/bash",
        "/usr/bin/rpm",
        "/usr/bin/rpmdb",
        "/usr/bin/rpmquery",
        "/usr/bin/rpmverify",
        "/usr/bin/rpmkeys",
    ] {
        files.insert(PathBuf::from(command));
        let output = std::process::Command::new("ldd")
            .arg(command)
            .output()
            .unwrap();
        assert!(output.status.success());
        for word in String::from_utf8(output.stdout).unwrap().split_whitespace() {
            if word.starts_with('/') {
                files.insert(PathBuf::from(word));
            }
        }
    }
    files.insert(PathBuf::from("/usr/lib/rpm/rpmrc"));
    files.insert(PathBuf::from("/usr/lib/rpm/macros"));
    for entry in std::fs::read_dir("/usr/lib/rpm").unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().starts_with("rpmpopt-") {
            files.insert(entry.path());
        }
    }
    for file in files {
        builder
            .with_file_contents(
                std::fs::read(&file).unwrap(),
                rpm::FileOptions::new(file.to_str().unwrap()).mode(0o100755),
            )
            .unwrap();
    }
    builder.build().unwrap().write_file(&path).unwrap();
    path
}
