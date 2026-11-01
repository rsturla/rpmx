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
fn automatic_root_reports_a_missing_offline_closure() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    let package = format!("rpmx-ux-missing-{}", std::process::id());
    write(
        &recipe,
        &format!(
            r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: 1
  summary: A probe package
  license: MIT
build:
  root:
    releasever: "44"
    archs: [x86_64]
    repositories:
      - id: probe
        baseurl: https://example.invalid/fedora
    packages: [{package}]
  pipeline:
    - runs: "true"
outputs:
  - name: probe
    metapackage: true
"#
        ),
    );
    let out = directory.path().join("out");
    let output = rpmx(&[
        "build",
        recipe.to_str().unwrap(),
        "--offline",
        "--output",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(value["ok"], false);
    assert!(
        value["diagnostics"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("not cached"),
        "{value}"
    );
}

#[test]
fn srpm_json_lists_typed_artifacts_with_paths() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    let sources = directory.path().join("sources");
    let out = directory.path().join("out");
    write(&recipe, VALID);
    std::fs::create_dir(&sources).expect("sources");
    std::fs::create_dir(&out).expect("out");
    let output = rpmx(&[
        "srpm",
        recipe.to_str().unwrap(),
        "--sources",
        sources.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(value["schema"], 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["command"], "srpm");
    let artifacts = value["artifacts"].as_array().expect("artifacts");
    assert_eq!(artifacts.len(), 1, "{value}");
    assert_eq!(artifacts[0]["kind"], "srpm");
    let path = artifacts[0]["path"].as_str().expect("path");
    assert!(Path::new(path).is_file(), "missing artifact {path}");
}

#[test]
fn failing_guest_step_exits_5_and_keeps_stdout_clean() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    write(
        &recipe,
        &VALID.replace(
            "build:\n  pipeline:\n    - runs: \"true\"\n",
            "source-assembly:\n  pipeline:\n    - runs: \"echo before-fail; exit 17\"\nbuild:\n  pipeline:\n    - runs: \"true\"\n",
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
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(value["schema"], 1);
    assert_eq!(value["ok"], false);
    assert_eq!(value["command"], "source");
    assert_eq!(value["diagnostics"][0]["severity"], "error");
    let message = value["diagnostics"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(message.contains("command:"), "{message}");
    assert!(message.contains("cwd:"), "{message}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("before-fail"),
        "guest output missing: {stderr}"
    );
    assert!(
        stderr.contains("rpmx: step"),
        "step header missing: {stderr}"
    );
}

#[test]
fn keep_workdir_preserves_a_failed_workspace() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    write(
        &recipe,
        &VALID.replace(
            "build:\n  pipeline:\n    - runs: \"true\"\n",
            "source-assembly:\n  pipeline:\n    - runs: \"echo before-fail; exit 17\"\nbuild:\n  pipeline:\n    - runs: \"true\"\n",
        ),
    );
    let out = directory.path().join("out");
    let output = rpmx(&[
        "source",
        recipe.to_str().unwrap(),
        "--unsafe-host-root",
        "--output",
        out.to_str().unwrap(),
        "--keep-workdir",
    ]);
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let path = stderr
        .lines()
        .find_map(|line| line.strip_prefix("work directory: "))
        .expect("work directory line");
    let path = Path::new(path.trim());
    assert!(path.is_dir(), "workspace was not kept: {}", path.display());
    std::fs::remove_dir_all(path).expect("remove kept workspace");
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
