use std::path::Path;
use std::process::{Command, Output};

fn rpmx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .expect("run rpmx")
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
}

const VALID: &str = r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: 1
  summary: A probe package
  license: MIT
build:
  pipeline:
    - runs: "true"
outputs:
  - name: probe
    metapackage: true
"#;

#[test]
fn lint_json_is_one_document_on_stdout() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    write(&recipe, VALID);
    let output = rpmx(&["lint", recipe.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(value["schema"], 1);
    assert_eq!(value["ok"], true);
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn recipe_errors_exit_2_and_emit_one_json_document() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    write(&recipe, "schema: 1\npackage:\n  name: probe\n");
    let output = rpmx(&["lint", recipe.to_str().unwrap(), "--json"]);
    assert_eq!(output.status.code(), Some(2));
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(value["schema"], 1);
    assert_eq!(value["ok"], false);
    assert_eq!(value["command"], "lint");
    assert_eq!(value["diagnostics"][0]["severity"], "error");
}

#[test]
fn root_resolution_errors_exit_4() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    write(&recipe, VALID);
    let output = rpmx(&["build", recipe.to_str().unwrap(), "--json"]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(value["ok"], false);
    assert_eq!(value["command"], "build");
}

#[test]
fn guest_output_goes_to_stderr_when_json_is_set() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    write(
        &recipe,
        &VALID.replace(
            "build:\n  pipeline:\n    - runs: \"true\"\n",
            "source-assembly:\n  pipeline:\n    - runs: \"echo guest-output\"\nbuild:\n  pipeline:\n    - runs: \"true\"\n",
        ),
    );
    let out = directory.path().join("out");
    let output = rpmx(&[
        "source",
        recipe.to_str().unwrap(),
        "--unsafe-host-root",
        "--output",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(value["schema"], 1);
    assert_eq!(value["ok"], true);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("guest-output"),
        "guest output missing from stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
