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
schema: 2
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
fn json_is_a_global_flag_on_either_side_of_the_subcommand() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    write(&recipe, VALID);
    for args in [
        vec!["--json", "explain", "policy", recipe.to_str().unwrap()],
        vec!["explain", "policy", recipe.to_str().unwrap(), "--json"],
    ] {
        let output = rpmx(&args);
        assert!(output.status.success(), "{output:?}");
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("one JSON document");
        assert_eq!(value["schema"], 1);
        assert_eq!(value["command"], "explain");
    }
}

#[test]
fn recipe_errors_exit_2_and_emit_one_json_document() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    write(&recipe, "schema: 2\npackage:\n  name: probe\n");
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
    let output = rpmx(&[
        "build",
        recipe.to_str().unwrap(),
        "--output",
        directory.path().join("out").to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(value["outcome"], "failed");
    assert_eq!(value["operation"], "build");
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
schema: 2
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
    assert_eq!(value["outcome"], "failed");
    assert!(
        value["primary"]["message"]
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
    assert_eq!(value["document"], "rpmx-run-report");
    assert_eq!(value["version"], 1);
    assert_eq!(value["outcome"], "passed");
    assert_eq!(value["operation"], "srpm");
    let artifacts = value["artifacts"]["srpms"]
        .as_array()
        .expect("SRPM artifacts");
    assert_eq!(artifacts.len(), 1, "{value}");
    let path = artifacts[0].as_str().expect("path");
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
    assert_eq!(value["document"], "rpmx-run-report");
    assert_eq!(value["version"], 1);
    assert_eq!(value["outcome"], "failed");
    assert_eq!(value["operation"], "source");
    assert!(value["primary"]["code"]
        .as_str()
        .is_some_and(|code| code.starts_with("RPMX-")));
    let message = value["primary"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("command:"), "{message}");
    assert!(message.contains("cwd:"), "{message}");
    assert!(message.contains("workspace:"), "{message}");
    assert!(message.contains("stage:"), "{message}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("before-fail"),
        "guest output missing: {stderr}"
    );
    assert!(
        stderr.contains("INFO step"),
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
        .find_map(|line| {
            line.strip_prefix("work directory (source): ")
                .or_else(|| line.strip_prefix("INFO work     source: "))
        })
        .expect("labelled work directory line");
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
    assert_eq!(value["document"], "rpmx-run-report");
    assert_eq!(value["version"], 1);
    assert_eq!(value["outcome"], "passed");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("guest-output"),
        "guest output missing from stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
