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
  maintainers:
    - name: A Maintainer
      email: a@example.org
build:
  pipeline:
    - uses: strip
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

#[test]
fn steps_list_and_show_expose_the_library() {
    let list = rpmx(&["steps", "list"]);
    assert!(list.status.success(), "{list:?}");
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(stdout.lines().any(|line| line == "cmake"), "{stdout}");
    assert!(stdout.lines().any(|line| line == "strip"), "{stdout}");

    let show = rpmx(&["steps", "show", "strip"]);
    assert!(show.status.success(), "{show:?}");
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.contains("name: strip"), "{stdout}");
    assert!(stdout.contains("needs:"), "{stdout}");
}

#[test]
fn explain_policy_is_a_versioned_document() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    std::fs::write(&recipe, RECIPE).expect("recipe");
    let output = rpmx(&["explain", "policy", recipe.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["schema"], 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["command"], "explain");
    assert_eq!(value["release"][0], "1");
}

#[test]
fn doctor_reports_checks_as_json() {
    let output = rpmx(&["doctor", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["schema"], 1);
    assert_eq!(value["command"], "doctor");
    assert!(value["checks"]
        .as_array()
        .is_some_and(|checks| !checks.is_empty()));
}

#[test]
fn shell_rejects_an_unbound_root_before_running_a_command() {
    let directory = tempfile::tempdir().expect("directory");
    let marker = directory.path().join("executed");
    let output = rpmx(&[
        "shell",
        "--root",
        "/",
        "--command",
        &format!("touch {}", marker.display()),
    ]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("--root requires --root-lock"));
    assert!(
        !marker.exists(),
        "command ran without verified root evidence"
    );
}

#[test]
fn plain_plan_displays_source_and_acceptance_cases() {
    let directory = tempfile::tempdir().expect("directory");
    let recipe = directory.path().join("package.yaml");
    let text = RECIPE.replace("build:\n", "build:\n  tests:\n    - name: upstream\n      packages: [make]\n      script: make check\n").replace("    metapackage: true", "    metapackage: true\n    tests:\n      - name: behavior\n        argv: [true]");
    std::fs::write(&recipe, text).expect("recipe");
    let output = rpmx(&["plan", recipe.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("build test") && text.contains("upstream"),
        "{text}"
    );
    assert!(
        text.contains("behavior") && text.contains("(installed)"),
        "{text}"
    );
    assert!(
        text.contains("make check") && text.contains("argv   true"),
        "{text}"
    );
}

#[test]
fn unknown_step_names_fail_with_a_hint() {
    let output = rpmx(&["steps", "show", "nope/nope"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("rpmx steps list"), "{stderr}");
}
