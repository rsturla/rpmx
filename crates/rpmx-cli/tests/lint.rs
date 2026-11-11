use std::process::{Command, Output};

const WEAK: &str = r#"
schema: 2
package:
  name: probe
  version: "1.0"
  release: 0
  summary: "a"
  license: "MIT OR"
build:
  packages: [gcc, gcc]
  pipeline:
    - runs: "true"
outputs:
  - name: probe
    metapackage: true
"#;

const CLEAN: &str = r#"
schema: 2
package:
  name: probe
  version: "1.0"
  release: 1
  summary: A probe package
  license: MIT
  source-date-epoch: 1704067200
build:
  packages: [gcc]
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

fn recipe(text: &str) -> (tempfile::TempDir, String) {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("package.yaml");
    std::fs::write(&path, text).expect("recipe");
    let path = path.display().to_string();
    (directory, path)
}

#[test]
fn strict_scripts_are_valid_and_shell_argv_is_rejected() {
    for (fields, valid) in [
        ("argv: [sh, -c, \"false\\ntrue\"]", false),
        ("script: \"false\\ntrue\"", true),
    ] {
        let (_directory, path) = recipe(&format!("{CLEAN}tests:\n  - name: probe\n    {fields}\n"));
        let output = rpmx(&["lint", &path]);
        assert_eq!(output.status.success(), valid, "{output:?}");
    }
}

#[test]
fn lint_reports_every_rule_in_one_run() {
    let (_directory, path) = recipe(WEAK);
    let output = rpmx(&["lint", &path]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("RPMX-LINT-010"), "{stdout}");
    assert!(stdout.contains("RPMX-LINT-011"), "{stdout}");
    assert!(stdout.contains("RPMX-LINT-012"), "{stdout}");
    assert!(stdout.contains("RPMX-LINT-014"), "{stdout}");
}

#[test]
fn lint_json_carries_every_diagnostic() {
    let (_directory, path) = recipe(WEAK);
    let output = rpmx(&["lint", &path, "--json"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["schema"], 1);
    assert_eq!(value["ok"], false);
    assert_eq!(value["command"], "lint");
    assert_eq!(value["summary"]["errors"], 3);
    assert_eq!(value["summary"]["warnings"], 3);
    let codes: Vec<_> = value["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().expect("code"))
        .collect();
    assert!(codes.contains(&"RPMX-LINT-010"));
    assert!(codes.contains(&"RPMX-LINT-011"));
    assert!(codes.contains(&"RPMX-LINT-012"));
    assert!(codes.contains(&"RPMX-LINT-014"));
}

#[test]
fn strict_turns_a_warning_into_a_failure() {
    let (_directory, path) = recipe(CLEAN);
    let relaxed = rpmx(&["lint", &path]);
    assert!(relaxed.status.success(), "{relaxed:?}");
    let strict = rpmx(&["lint", &path, "--strict"]);
    assert_eq!(strict.status.code(), Some(2), "{strict:?}");
    let stdout = String::from_utf8_lossy(&strict.stdout);
    assert!(stdout.contains("RPMX-LINT-019"), "{stdout}");
}
