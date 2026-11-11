use rpmx_application::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn available() -> bool {
    Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "true"])
        .status()
        .is_ok_and(|status| status.success())
}
fn executable(root: &Path, command: &str) {
    let output = Command::new("ldd").arg(command).output().unwrap();
    assert!(output.status.success(), "ldd {command}");
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
fn build_root(directory: &Path) -> BuildRoot {
    use rpmx_root::root::*;
    let root = directory.join("root");
    for command in [
        "/usr/bin/bash",
        "/usr/bin/sh",
        "/usr/bin/mkdir",
        "/usr/bin/ln",
        "/usr/bin/cat",
    ] {
        executable(&root, command);
    }
    for path in ["tmp", "dev", "proc", "sys"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    let arch = rpmx_base::arch::native_arch();
    let packages = std::collections::BTreeMap::from([(
        arch.into(),
        ["bash", "coreutils"]
            .into_iter()
            .map(|name| LockedPackage {
                name: name.into(),
                origin: PackageOrigin::Candidate,
                evr: "1-1".into(),
                checksum: format!("sha512:{}", "a".repeat(128)),
                size: 1,
                srpm: None,
                module: None,
            })
            .collect(),
    )]);
    // This deliberately assembled test root exercises execution and binding;
    // resolver conformance has separate real RPM transaction controls.
    let repositories = vec![rpmx_root::profile::Repository {
        id: "private".into(),
        baseurl: Some("https://example.invalid/private".into()),
        metalink: None,
        mirrorlist: None,
        gpgkey: None,
        repo_gpgcheck: false,
        exclude: Vec::new(),
        priority: None,
        auth: Some(
            serde_json::from_str(
                r#"{"username":"private-user","password":"env:PRIVATE_REPOSITORY_SECRET"}"#,
            )
            .unwrap(),
        ),
    }];
    let lock = RootLock {
        document: DOCUMENT.into(),
        version: VERSION,
        profile: "execution-fixture".into(),
        archs: vec![arch.into()],
        releasever: "44".into(),
        dist_tag: None,
        options: Default::default(),
        prefix: "/usr".into(),
        libdir: "/usr/lib64".into(),
        sbom: false,
        closure_digest: closure_digest("44", &[arch.into()], &repositories, &packages),
        repositories,
        packages,
        root_digest: Some(rpmx_store::tree::digest_root(&root).unwrap()),
        rpmdb_digest: Some(rpmx_store::tree::rpmdb_digest(&root).unwrap()),
        builder: None,
        seed: None,
        generation: None,
        requirement: None,
    };
    let path = directory.join("root.lock.yaml");
    write_lock(&path, &lock).unwrap();
    BuildRoot::Locked {
        lock: path,
        root: Some(root),
    }
}
fn recipe(directory: &Path, fail: bool) -> PathBuf {
    let path = directory.join("package.yaml");
    let yaml = r#"
schema: 2
package:
  name: probe
  version: "1"
  release: 1
  summary: production snapshot probe
  license: MIT
build:
  pipeline:
    - name: stage payload and component metadata
      runs: |
        mkdir -p "$STAGE/usr/share/probe"
        printf 'production bytes\n' >"$STAGE/usr/share/probe/data"
        printf 'production bytes\n' >source-alias
        printf 'frozen-dependency=1\n' >"$RPMX_BUNDLED_DIR/probe/bits.list"
  tests:
    - name: source test with mutations
      script: |
        test "$(cat "$STAGE/usr/share/probe/data")" = 'production bytes'
        if printf wrong >"$STAGE/usr/share/probe/data"; then exit 71; fi
        printf 'changed test copy\n' >source-alias
        printf 'frozen-dependency=9\n' >"$RPMX_BUNDLED_DIR/probe/bits.list"
        printf 'retained test evidence\n' >result.log
        @RESULT@
      artifacts: [result.log]
outputs:
  - name: probe
    files:
      include: [/usr/share/probe/data]
tests:
  - name: acceptance is standalone
    script: exit 99
"#
    .replace("@RESULT@", if fail { "exit 7" } else { "true" });
    std::fs::write(&path, yaml).unwrap();
    path
}
fn input(recipe: PathBuf) -> BuildInput {
    BuildInput {
        recipe,
        architectures: vec![rpmx_base::arch::native_arch().into()],
        layout: BuildOutputLayout::Architectures,
        arch_jobs: None,
        cell: None,
        stage: None,
        build_tests: true,
        locked: false,
        lint: false,
        index: false,
        sbom: None,
    }
}
#[test]
fn build_tests_cannot_change_production_bytes_or_component_metadata() {
    if !available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let config = ApplicationConfig {
        root: build_root(directory.path()),
        runner: "bwrap".into(),
        quiet: true,
        ..Default::default()
    };
    let output = directory.path().join("output");
    let report = Application::new(config)
        .execute(ApplicationRequest {
            operation: Operation::Build(input(recipe(directory.path(), false))),
            output,
        })
        .unwrap();
    assert!(
        report.success(),
        "{:?}, {:?}",
        report.primary,
        report.secondary
    );
    assert_eq!(report.build_tests.len(), 1);
    assert_eq!(report.build_tests[0].status, CaseStatus::Passed);
    assert_eq!(report.acceptance, AcceptanceState::NotRequested);
    assert!(report.tests.is_empty());
    assert_eq!(report.artifacts.rpms.len(), 1);
    let package = rpmx_rpm::package_metadata(&report.artifacts.rpms[0]).unwrap();
    assert!(package
        .provides
        .iter()
        .any(|dep| dep.name == "bundled(frozen-dependency)" && dep.version == "1"));
    assert!(!package
        .provides
        .iter()
        .any(|dep| dep.name == "bundled(frozen-dependency)" && dep.version == "9"));
    let extracted = directory.path().join("extracted");
    rpmx_payload::extract_payload(
        &rpm::Package::open(&report.artifacts.rpms[0]).unwrap(),
        &extracted,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(extracted.join("usr/share/probe/data")).unwrap(),
        "production bytes\n"
    );
    assert!(report.build_tests[0]
        .evidence
        .join("artifacts/result.log")
        .is_file());
    let source_payload = directory.path().join("source-payload");
    rpmx_payload::extract_payload(
        &rpm::Package::open(&report.artifacts.srpms[0]).unwrap(),
        &source_payload,
    )
    .unwrap();
    let public = std::fs::read_to_string(source_payload.join("root.lock.json")).unwrap();
    assert!(!public.contains("PRIVATE_REPOSITORY_SECRET"));
    assert!(!public.contains("private-user"));
    let public: rpmx_root::root::RootLock = serde_json::from_str(&public).unwrap();
    public.validate().unwrap();
    assert_eq!(public.packages.values().map(Vec::len).sum::<usize>(), 2);
    let private = rpmx_root::root::load_lock(&directory.path().join("root.lock.yaml")).unwrap();
    assert!(
        private.repositories[0].auth.is_some(),
        "publication must not change private acquisition credentials"
    );
}
#[test]
fn failed_build_test_keeps_evidence_and_never_promotes_candidate_rpms() {
    if !available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let config = ApplicationConfig {
        root: build_root(directory.path()),
        runner: "bwrap".into(),
        quiet: true,
        ..Default::default()
    };
    let output = directory.path().join("output");
    let report = Application::new(config)
        .execute(ApplicationRequest {
            operation: Operation::Build(input(recipe(directory.path(), true))),
            output: output.clone(),
        })
        .unwrap();
    assert!(!report.success());
    assert_eq!(report.build_tests.len(), 1);
    assert_eq!(report.build_tests[0].status, CaseStatus::Failed);
    assert!(report.artifacts.rpms.is_empty());
    assert!(report.build_tests[0]
        .evidence
        .join("artifacts/result.log")
        .is_file());
    let native_output = output.join(rpmx_base::arch::native_arch());
    assert!(!std::fs::read_dir(native_output).unwrap().any(|entry| entry
        .unwrap()
        .path()
        .extension()
        .is_some_and(|extension| extension == "rpm")));
    assert!(output.join("run-report.json").is_file());
    assert!(output.join("run-report.xml").is_file());
}

#[test]
fn skipped_requested_build_test_never_publishes_and_needs_no_test_environment() {
    if !available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let config = ApplicationConfig {
        root: build_root(directory.path()),
        runner: "bwrap".into(),
        quiet: true,
        ..Default::default()
    };
    let recipe = recipe(directory.path(), false);
    let yaml = std::fs::read_to_string(&recipe).unwrap().replace(
        "    - name: source test with mutations",
        "    - name: source test with mutations\n      skip: not available on this platform",
    );
    std::fs::write(&recipe, yaml).unwrap();
    let report = Application::new(config)
        .execute(ApplicationRequest::build(
            recipe,
            directory.path().join("output"),
        ))
        .unwrap();
    assert!(
        !report.success(),
        "skipped tests must not publish passing artifacts"
    );
    assert!(report.artifacts.rpms.is_empty());
    assert_eq!(report.build_tests.len(), 1);
    assert_eq!(report.build_tests[0].status, CaseStatus::Skipped);
    assert!(
        report.environments.is_empty(),
        "a skipped case must not prepare an environment"
    );
}

#[test]
fn offline_missing_test_environment_is_reported_before_the_build_runs() {
    if !available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let config = ApplicationConfig {
        root: build_root(directory.path()),
        cache: Some(directory.path().join("cas")),
        offline: true,
        runner: "bwrap".into(),
        quiet: true,
        ..Default::default()
    };
    let recipe = recipe(directory.path(), false);
    let yaml = std::fs::read_to_string(&recipe)
        .unwrap()
        .replace(
            "        mkdir -p",
            "        exit 89 # the production pipeline must never start\n        mkdir -p",
        )
        .replace(
            "    - name: source test with mutations",
            "    - name: source test with mutations\n      packages: [unavailable-check-tool]",
        );
    std::fs::write(&recipe, yaml).unwrap();
    let report = Application::new(config)
        .execute(ApplicationRequest::build(
            recipe,
            directory.path().join("output"),
        ))
        .unwrap();
    assert!(!report.success());
    assert!(report.artifacts.rpms.is_empty());
    let error = report.primary.unwrap();
    assert_eq!(error.code, "RPMX-TEST-005");
    assert!(error.message.contains("not cached"), "{}", error.message);
    assert_eq!(report.build_tests[0].status, CaseStatus::Failed);
    assert_eq!(report.environments[0].status, RunOutcome::Failed);
    assert!(report.build_tests[0]
        .evidence
        .join("preparation.log")
        .is_file());
}

#[test]
fn explicitly_disabling_build_tests_does_not_run_either_test_phase() {
    if !available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let config = ApplicationConfig {
        root: build_root(directory.path()),
        runner: "bwrap".into(),
        quiet: true,
        ..Default::default()
    };
    let mut input = input(recipe(directory.path(), true));
    input.build_tests = false;
    let report = Application::new(config)
        .execute(ApplicationRequest {
            operation: Operation::Build(input),
            output: directory.path().join("output"),
        })
        .unwrap();
    assert!(report.success(), "{:?}", report.primary);
    assert!(!report.build_tests_requested);
    assert!(report.build_tests.is_empty());
    assert!(report.tests.is_empty());
    assert_eq!(report.artifacts.rpms.len(), 1);
}

#[test]
fn failed_source_assembly_retains_both_streams_and_reports_original_failure() {
    if !available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let config = ApplicationConfig {
        root: build_root(directory.path()),
        runner: "bwrap".into(),
        quiet: true,
        ..Default::default()
    };
    let recipe = recipe(directory.path(), false);
    let yaml = std::fs::read_to_string(&recipe).unwrap().replace("build:\n", "source-assembly:\n  pipeline:\n    - runs: |\n        printf 'source preparation failed\\n'\n        printf 'source diagnostic\\n' >&2\n        exit 7\nbuild:\n");
    std::fs::write(&recipe, yaml).unwrap();
    let report = Application::new(config)
        .execute(ApplicationRequest::build(
            recipe,
            directory.path().join("output"),
        ))
        .unwrap();
    assert!(!report.success());
    let error = report.primary.unwrap();
    assert!(error.message.contains("7"), "{}", error.message);
    assert!(report.artifacts.rpms.is_empty());
    assert!(report.artifacts.sources.is_empty());
    let evidence = report
        .evidence
        .iter()
        .find(|path| path.join("stdout.log").exists())
        .expect("source evidence registered before execution");
    assert!(std::fs::read_to_string(evidence.join("stdout.log"))
        .unwrap()
        .contains("source preparation failed"));
    assert!(std::fs::read_to_string(evidence.join("stderr.log"))
        .unwrap()
        .contains("source diagnostic"));
}

#[test]
fn staged_packaging_requires_explicit_opt_out_of_declared_build_tests() {
    let directory = tempfile::tempdir().unwrap();
    let stage = directory.path().join("stage");
    std::fs::create_dir_all(stage.join("usr/share/probe")).unwrap();
    std::fs::write(stage.join("usr/share/probe/data"), "production bytes").unwrap();
    let mut input = input(recipe(directory.path(), false));
    input.stage = Some(stage);
    let report = Application::new(ApplicationConfig::default())
        .execute(ApplicationRequest {
            operation: Operation::Build(input),
            output: directory.path().join("output"),
        })
        .unwrap();
    assert!(!report.success());
    assert!(
        !report.build_tests_requested,
        "package-only operation cannot request a build workspace test"
    );
    assert!(report.artifacts.rpms.is_empty());
    let error = report.primary.unwrap();
    assert_eq!(error.code, "RPMX-TEST-005");
    assert!(error.message.contains("--no-tests"));
}

#[test]
fn staged_packaging_retains_mode_zero_payload_without_changing_its_source() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let stage = directory.path().join("stage");
    let source = stage.join("usr/share/probe/data");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "protected production bytes").unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o0)).unwrap();
    let mut input = input(recipe(directory.path(), false));
    input.stage = Some(stage);
    input.build_tests = false;
    let report = Application::new(ApplicationConfig::default())
        .execute(ApplicationRequest {
            operation: Operation::Build(input),
            output: directory.path().join("output"),
        })
        .unwrap();
    assert!(report.success(), "{:?}", report.primary);
    let package = rpm::Package::open(&report.artifacts.rpms[0]).unwrap();
    let files = package.metadata.get_file_entries().unwrap();
    let entry = files
        .iter()
        .find(|entry| entry.path() == Path::new("/usr/share/probe/data"))
        .unwrap();
    assert_eq!(
        entry.mode().raw_mode(),
        0o100000,
        "the RPM header must retain regular-file type and mode000"
    );
    let payload = directory.path().join("payload");
    rpmx_payload::extract_payload(&package, &payload).unwrap();
    assert_eq!(
        std::fs::metadata(payload.join("usr/share/probe/data"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o0,
        "the extracted candidate view must retain mode000 too"
    );
    let contents = Command::new("unshare")
        .args(["--user", "--map-root-user", "--", "cat"])
        .arg(payload.join("usr/share/probe/data"))
        .output()
        .unwrap();
    assert!(
        contents.status.success(),
        "{}",
        String::from_utf8_lossy(&contents.stderr)
    );
    assert_eq!(contents.stdout, b"protected production bytes");
    assert_eq!(
        std::fs::metadata(&source).unwrap().permissions().mode() & 0o7777,
        0o0,
        "packaging must not chmod the staged source"
    );
}
