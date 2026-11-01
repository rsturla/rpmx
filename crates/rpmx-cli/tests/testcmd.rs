use std::process::{Command, Output};

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

const BASE: &str = r#"
schema: 1
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
    - runs: "true"
outputs:
  - name: probe
    metapackage: true
"#;

#[test]
fn a_recipe_without_tests_is_a_recipe_error() {
    let (_directory, path) = recipe(BASE);
    let output = rpmx(&["test", &path]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("RPMX-TEST-006"), "{stderr}");
}

#[test]
fn tests_without_a_root_are_a_resolution_error() {
    let (_directory, path) = recipe(&format!(
        "{}tests:\n  - name: smoke\n    stage: build\n    argv: [\"true\"]\n",
        BASE
    ));
    let output = rpmx(&["test", &path]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("RPMX-ROOT-016"), "{stderr}");
}
