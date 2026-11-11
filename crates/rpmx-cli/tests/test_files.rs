use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

fn rpmx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .unwrap()
}

fn recipe(directory: &Path, tests: &str) -> std::path::PathBuf {
    let path = directory.join("package.yaml");
    std::fs::write(
        &path,
        format!(
            r#"
schema: 1
package:
  name: fixture-probe
  version: "1"
  release: 1
  summary: Fixture probe
  license: MIT
  source-date-epoch: 1704067200
build:
  pipeline: []
outputs:
  - name: fixture-probe
    metapackage: true
tests:
{tests}
"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn fixtures_survive_the_srpm_and_change_the_recipe_digest() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("tests/include")).unwrap();
    let fixture = directory.path().join("tests/include/probe.h");
    std::fs::write(&fixture, "#define ANSWER 42\n").unwrap();
    let path = recipe(
        directory.path(),
        "  - name: headers\n    stage: installed\n    files: tests\n    argv: ['true']",
    );
    let before = rpmx_core::load_recipe_bundle(&path).unwrap();
    std::fs::create_dir(directory.path().join("sources")).unwrap();
    let output = rpmx(&[
        "srpm",
        path.to_str().unwrap(),
        "--sources",
        directory.path().join("sources").to_str().unwrap(),
        "--output",
        directory.path().join("out").to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    let srpm = std::fs::read_dir(directory.path().join("out"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".src.rpm"))
        .unwrap();
    let extracted = directory.path().join("extracted");
    rpmx_rpm::extract::extract_srpm(&srpm, &extracted).unwrap();
    assert_eq!(
        std::fs::read(extracted.join("tests/include/probe.h")).unwrap(),
        std::fs::read(&fixture).unwrap()
    );
    let after = rpmx_core::load_recipe_bundle(extracted.join("package.yaml")).unwrap();
    assert_eq!(before.digest(), after.digest());
    std::fs::write(fixture, "#define ANSWER 0\n").unwrap();
    assert_ne!(
        before.digest(),
        rpmx_core::load_recipe_bundle(path).unwrap().digest()
    );
}

fn host_lock(path: &Path) {
    let arch = std::env::consts::ARCH;
    let packages = BTreeMap::from([(
        arch.to_string(),
        Vec::<rpmx_core::root::LockedPackage>::new(),
    )]);
    let digest = rpmx_core::root::closure_digest("test", &[arch.to_string()], &[], &packages);
    let lock = serde_json::json!({"document":"rpmx-root-lock", "version":1, "profile":"host-test", "archs":[arch], "releasever":"test", "repositories":[], "packages":packages, "closure_digest":digest});
    std::fs::write(path, serde_yaml_ng::to_string(&lock).unwrap()).unwrap();
}

#[test]
fn installed_tests_compile_external_files_in_separate_workspaces() {
    if !Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--", "true"])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        eprintln!("bubblewrap is unavailable; skip the execution test");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("tests/include")).unwrap();
    std::fs::write(
        directory.path().join("tests/include/value.h"),
        "#define VALUE 42\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("tests/main.c"),
        "#include \"value.h\"\nint main(void) { return VALUE != 42; }\n",
    )
    .unwrap();
    let path = recipe(
        directory.path(),
        r#"  - name: compile
    stage: installed
    files: tests
    script: |
      test ! -e leaked
      cc -I"$TEST_FILES/include" "$TEST_FILES/main.c" -o probe
      ./probe
      touch leaked
      printf changed > "$TEST_FILES/main.c"
  - name: isolated
    stage: installed
    files: tests
    script: |
      test ! -e leaked
      test -z "$(ls -A "$STAGE")"
      cc -I"$TEST_FILES/include" "$TEST_FILES/main.c" -o probe
      ./probe
"#,
    );
    let lock = directory.path().join("root.lock.yaml");
    host_lock(&lock);
    let output = rpmx(&[
        "test",
        path.to_str().unwrap(),
        "--test-root",
        "/",
        "--test-output",
        directory.path().join("results").to_str().unwrap(),
        "--test-root-lock",
        lock.to_str().unwrap(),
        "--runner",
        "bwrap",
        "--offline",
    ]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        !std::fs::read_to_string(directory.path().join("tests/main.c"))
            .unwrap()
            .contains("changed")
    );
}

#[test]
fn matrix_cells_keep_distinct_logs_artifacts_and_report_metadata() {
    if !Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--", "true"])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        eprintln!("bubblewrap is unavailable; skip the execution test");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("package.yaml");
    std::fs::write(
        &path,
        r#"schema: 1
package:
  name: matrix-probe-${matrix.flavor}
  version: "1"
  release: 1
  summary: Matrix report probe
  license: MIT
  source-date-epoch: 1704067200
matrix:
  axes:
    flavor: [first, second]
outputs:
  - name: matrix-probe-${matrix.flavor}
    metapackage: true
tests:
  - name: shared
    stage: installed
    artifacts: [marker.txt]
    script: |
      printf 'stdout-${matrix.flavor}\n'
      printf 'stderr-${matrix.flavor}\n' >&2
      printf 'artifact-${matrix.flavor}\n' > marker.txt
"#,
    )
    .unwrap();
    let lock = directory.path().join("root.lock.yaml");
    host_lock(&lock);
    let junit = directory.path().join("junit.xml");
    let output = rpmx(&[
        "test",
        path.to_str().unwrap(),
        "--test",
        "shared",
        "--test-root",
        "/",
        "--test-root-lock",
        lock.to_str().unwrap(),
        "--test-output",
        directory.path().join("results").to_str().unwrap(),
        "--junit",
        junit.to_str().unwrap(),
        "--runner",
        "bwrap",
        "--offline",
        "--json",
    ]);
    assert!(output.status.success(), "{output:?}");
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tests = document["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 2);
    assert_ne!(tests[0]["results"], tests[1]["results"]);
    let xml = std::fs::read_to_string(junit).unwrap();
    assert_eq!(xml.matches("<testcase ").count(), 2);
    let mut cells = std::collections::BTreeSet::new();
    for test in tests {
        assert_eq!(test["name"], "shared", "selectors keep their recipe names");
        assert_eq!(test["status"], "passed");
        let cell = test["cell_id"].as_str().unwrap();
        cells.insert(cell);
        let flavor = cell.strip_prefix("flavor=").unwrap();
        assert!(["first", "second"].contains(&flavor));
        let other = if flavor == "first" { "second" } else { "first" };
        let results = Path::new(test["results"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(results.join("stdout.log")).unwrap(),
            format!("stdout-{flavor}\n")
        );
        let stderr = std::fs::read_to_string(results.join("stderr.log")).unwrap();
        assert!(stderr.contains(&format!("stderr-{flavor}\n")));
        assert!(!stderr.contains(&format!("stderr-{other}\n")));
        assert_eq!(
            std::fs::read_to_string(results.join("artifacts/marker.txt")).unwrap(),
            format!("artifact-{flavor}\n")
        );
        let case = xml
            .split("<testcase ")
            .skip(1)
            .find(|case| case.contains(&format!("classname=\"installed.{cell}\"")))
            .expect("JUnit identifies the matrix cell");
        assert!(case.contains(&format!("<system-out>stdout-{flavor}\n</system-out>")));
        assert!(case.contains(&format!("stderr-{flavor}\n")));
        assert!(!case.contains(&format!("stdout-{other}\n")));
        assert!(!case.contains(&format!("stderr-{other}\n")));
    }
    assert_eq!(
        cells,
        std::collections::BTreeSet::from(["flavor=first", "flavor=second"])
    );
}

#[test]
fn reports_keep_logs_artifacts_and_distinct_outcomes() {
    if !Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--", "true"])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = recipe(
        directory.path(),
        r#"  - name: assertion
    stage: installed
    artifacts: [upstream.xml]
    script: |
      printf '<testsuite name="upstream"/>\n' > upstream.xml
      printf 'out\n'
      printf 'err\n' >&2
      false
  - name: infrastructure
    stage: installed
    environment:
      JOBS: invalid
    script: 'true'
  - name: disabled
    stage: installed
    skip: Needs a kernel feature
    script: 'false'
  - name: success
    stage: installed
    script: 'true'
"#,
    );
    let lock = directory.path().join("root.lock.yaml");
    host_lock(&lock);
    let results = directory.path().join("results");
    let junit = directory.path().join("junit.xml");
    let output = rpmx(&[
        "test",
        path.to_str().unwrap(),
        "--test-root",
        "/",
        "--test-root-lock",
        lock.to_str().unwrap(),
        "--test-output",
        results.to_str().unwrap(),
        "--junit",
        junit.to_str().unwrap(),
        "--json",
        "--offline",
    ]);
    assert_eq!(output.status.code(), Some(6), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let statuses: Vec<_> = value["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|test| test["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["failed", "error", "skipped", "passed"]);
    let failed = Path::new(value["tests"][0]["results"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(failed.join("stdout.log")).unwrap(),
        "out\n"
    );
    assert_eq!(
        std::fs::read_to_string(failed.join("stderr.log")).unwrap(),
        "err\n"
    );
    assert!(failed.join("artifacts/upstream.xml").is_file());
    let xml = std::fs::read_to_string(junit).unwrap();
    for expected in [
        "failures=\"1\"",
        "errors=\"1\"",
        "skipped=\"1\"",
        "<system-out>out",
        "<system-err>err",
    ] {
        assert!(xml.contains(expected), "{xml}");
    }
}

#[test]
fn an_explicit_skip_needs_no_root_and_setup_errors_get_reports() {
    let directory = tempfile::tempdir().unwrap();
    let path = recipe(directory.path(), "  - name: skipped\n    stage: installed\n    skip: Needs a kernel feature\n    script: 'false'");
    let output = rpmx(&["test", path.to_str().unwrap(), "--json", "--offline"]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["tests"][0]["status"], "skipped");
    let path = recipe(
        directory.path(),
        "  - name: missing-root\n    stage: installed\n    script: 'true'",
    );
    let report = directory.path().join("setup.xml");
    let output = rpmx(&[
        "test",
        path.to_str().unwrap(),
        "--offline",
        "--junit",
        report.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(std::fs::read_to_string(report).unwrap().contains("<error "));
}

#[test]
fn environment_reports_survive_inputs_and_keep_json_stdout_clean() {
    if !Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--", "true"])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }
    let inputs = tempfile::tempdir().unwrap();
    let reports = tempfile::tempdir().unwrap();
    let path = recipe(
        inputs.path(),
        "  - name: success\n    stage: installed\n    argv: ['true']",
    );
    let lock_path = inputs.path().join("root.lock.yaml");
    host_lock(&lock_path);
    let mut lock = rpmx_core::root::load_lock(&lock_path).unwrap();
    lock.repositories.push(rpmx_core::profile::Repository {
        id: "private".into(),
        baseurl: Some("https://example.invalid/packages".into()),
        metalink: None,
        mirrorlist: None,
        gpgkey: None,
        repo_gpgcheck: false,
        exclude: Vec::new(),
        priority: None,
        auth: Some(rpmx_core::profile::RepositoryAuth {
            username: Some("test-user".into()),
            password: Some("test-secret".into()),
            ..Default::default()
        }),
    });
    lock.closure_digest = rpmx_core::root::closure_digest(
        &lock.releasever,
        &lock.archs,
        &lock.repositories,
        &lock.packages,
    );
    rpmx_core::root::write_lock(&lock_path, &lock).unwrap();
    let stale = reports.path().join("environments/stale/root.lock.yaml");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "stale lock").unwrap();
    let output = rpmx(&[
        "test",
        path.to_str().unwrap(),
        "--test-root",
        "/",
        "--test-root-lock",
        lock_path.to_str().unwrap(),
        "--test-output",
        reports.path().to_str().unwrap(),
        "--json",
        "--offline",
    ]);
    assert!(output.status.success(), "{output:?}");
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["ok"], true);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("using saved test environment 1/1: output=all outputs; tools=none"));
    assert!(!stderr.contains("test-secret"));
    drop(inputs);
    assert!(!stale.exists());
    let evidence = reports.path().join("environments/1");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(evidence.join("environment.json")).unwrap()).unwrap();
    assert_eq!(metadata["status"], "ready");
    assert_eq!(metadata["tests"], serde_json::json!(["success"]));
    let saved_text = std::fs::read_to_string(evidence.join("root.lock.yaml")).unwrap();
    assert!(!saved_text.contains("test-secret") && !saved_text.contains("test-user"));
    let saved = rpmx_core::root::load_lock(&evidence.join("root.lock.yaml")).unwrap();
    assert_eq!(saved.closure_digest, lock.closure_digest);
    assert_eq!(
        saved.packages.keys().collect::<Vec<_>>(),
        lock.packages.keys().collect::<Vec<_>>()
    );
}

#[test]
fn environment_setup_errors_replace_stale_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let path = recipe(
        directory.path(),
        "  - name: needs-image\n    stage: installed\n    argv: ['true']",
    );
    let stage = directory.path().join("stage");
    std::fs::create_dir(&stage).unwrap();
    let artifacts = directory.path().join("rpms");
    let built = rpmx(&[
        "build",
        path.to_str().unwrap(),
        "--stage",
        stage.to_str().unwrap(),
        "--output",
        artifacts.to_str().unwrap(),
        "--no-tests",
    ]);
    assert!(built.status.success(), "{built:?}");
    let lock = directory.path().join("root.lock.yaml");
    host_lock(&lock);
    let reports = directory.path().join("results");
    let stale = reports.join("environments/1/root.lock.yaml");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "stale lock").unwrap();
    let output = rpmx(&[
        "test",
        path.to_str().unwrap(),
        "--root-lock",
        lock.to_str().unwrap(),
        "--install-dir",
        artifacts.join(std::env::consts::ARCH).to_str().unwrap(),
        "--test-output",
        reports.to_str().unwrap(),
        "--json",
    ]);
    assert!(!output.status.success(), "{output:?}");
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["diagnostics"][0]["code"], "RPMX-BUILD-007");
    assert!(document["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("builder image"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("preparing test environment 1/1: output=all outputs; tools=none"));
    assert!(!stale.exists());
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(reports.join("environments/1/environment.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(metadata["status"], "error");
    assert_eq!(metadata["error_code"], "RPMX-BUILD-007");
}
