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
