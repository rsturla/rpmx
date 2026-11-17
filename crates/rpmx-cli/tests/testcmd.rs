use std::path::Path;
use std::process::{Command, Output};

fn invoke(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn test_requires_existing_rpms_and_never_implicitly_builds() {
    let directory = tempfile::tempdir().unwrap();
    let output = invoke(directory.path(), &["test", "missing.yaml"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--install-dir"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn preparation_failure_has_one_json_document_and_persisted_reports() {
    let directory = tempfile::tempdir().unwrap();
    let output = invoke(
        directory.path(),
        &[
            "test",
            "missing.yaml",
            "--install-dir",
            "missing-rpms",
            "--test-output",
            "results",
            "--junit",
            "junit.xml",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout contains exactly one JSON document");
    assert_eq!(report["document"], "rpmx-run-report");
    assert_eq!(report["outcome"], "failed");
    assert!(report["primary"]["message"]
        .as_str()
        .unwrap()
        .contains("missing-rpms"));
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("results/run-report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, report);
    assert_eq!(
        std::fs::read(directory.path().join("junit.xml")).unwrap(),
        std::fs::read(directory.path().join("results/run-report.xml")).unwrap()
    );
}

#[test]
fn junit_copy_failure_preserves_the_original_failure_in_the_same_report() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("blocked"), "not a directory").unwrap();
    let output = invoke(
        directory.path(),
        &[
            "test",
            "missing.yaml",
            "--install-dir",
            "missing-rpms",
            "--test-output",
            "results",
            "--junit",
            "blocked/report.xml",
            "--json",
        ],
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one populated run report");
    assert_eq!(report["document"], "rpmx-run-report");
    assert!(report["primary"]["message"]
        .as_str()
        .unwrap()
        .contains("missing-rpms"));
    assert!(report["secondary"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["message"].as_str().unwrap().contains("JUnit")));
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("results/run-report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, report);
}

#[test]
fn plan_marks_optional_cases_without_hiding_mandatory_ones() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("package.yaml"),
        r#"
schema: 2
package:
  name: example
  version: '1'
  release: '1'
  summary: Example
  license: MIT
build:
  pipeline:
    - runs: 'true'
  tests:
    - name: upstream
      optional: true
      script: 'true'
outputs:
  - name: example
    tests:
      - name: behavior
        script: 'true'
      - name: extended
        optional: true
        script: 'true'
"#,
    )
    .unwrap();
    let output = invoke(directory.path(), &["plan", "--arch", "x86_64"]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("build test upstream [optional]"), "{text}");
    assert!(
        text.contains("example/extended (installed) [optional]"),
        "{text}"
    );
    assert!(text.contains("example/behavior (installed)\n"), "{text}");
}
