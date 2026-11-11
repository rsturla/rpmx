use std::path::Path;
use std::process::{Command, Output};

const RECIPE: &str = r#"
schema: 2
package:
  name: probe
  version: "1.0"
  release: 1
  summary: A probe package
  license: MIT
  source-date-epoch: 1704067200
build:
  pipeline:
    - runs: "true"
outputs:
  - name: probe
    metapackage: true
"#;

fn rpmx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .expect("run rpmx")
}

fn write_recipe(directory: &Path, version: &str) -> std::path::PathBuf {
    let path = directory.join("package.yaml");
    std::fs::write(&path, RECIPE.replace("\"1.0\"", &format!("\"{version}\""))).expect("recipe");
    path
}

#[test]
fn lock_writes_a_reviewable_document_and_checks_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = write_recipe(directory.path(), "1.0");
    let output = rpmx(&["lock", recipe.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    let lock = directory.path().join("package.lock.json");
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock).expect("lock")).expect("json");
    assert_eq!(value["schema"], 1);
    assert!(value["recipe_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha512:"));
    assert_eq!(value["cells"][0]["package"], "probe");
    assert_eq!(value["cells"][0]["version"], "1.0");

    let check = rpmx(&["lock", recipe.to_str().unwrap(), "--check"]);
    assert!(check.status.success(), "{check:?}");
}

#[test]
fn lock_check_reports_recipe_drift_with_exit_3() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = write_recipe(directory.path(), "1.0");
    assert!(rpmx(&["lock", recipe.to_str().unwrap()]).status.success());
    write_recipe(directory.path(), "1.1");
    let check = rpmx(&["lock", recipe.to_str().unwrap(), "--check"]);
    assert_eq!(check.status.code(), Some(3), "{check:?}");
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(stderr.contains("RPMX-LOCK-002"), "{stderr}");
    assert!(stderr.contains("changed: recipe"), "{stderr}");
}

#[test]
fn a_locked_build_refuses_to_start_on_drift() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = write_recipe(directory.path(), "1.0");
    assert!(rpmx(&["lock", recipe.to_str().unwrap()]).status.success());
    write_recipe(directory.path(), "1.1");
    let build = rpmx(&[
        "build",
        recipe.to_str().unwrap(),
        "--locked",
        "--output",
        directory.path().join("out").to_str().unwrap(),
    ]);
    assert_eq!(build.status.code(), Some(3), "{build:?}");
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(stderr.contains("RPMX-LOCK-002"), "{stderr}");
}

#[test]
fn a_locked_build_without_a_lock_is_a_prerequisite_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = write_recipe(directory.path(), "1.0");
    let build = rpmx(&[
        "build",
        recipe.to_str().unwrap(),
        "--locked",
        "--output",
        directory.path().join("out").to_str().unwrap(),
    ]);
    assert_eq!(build.status.code(), Some(2), "{build:?}");
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(stderr.contains("RPMX-LOCK-003"), "{stderr}");
}
