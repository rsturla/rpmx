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
    - runs: "true"
outputs:
  - name: probe
    metapackage: true
"#;

fn rpmx_in(directory: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .current_dir(directory)
        .args(args)
        .output()
        .expect("run rpmx")
}

#[test]
fn lint_defaults_to_package_yaml_in_the_current_directory() {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::write(directory.path().join("package.yaml"), RECIPE).expect("recipe");
    let output = rpmx_in(directory.path(), &["lint"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "ok: 1 cell(s): default"
    );
}

#[test]
fn build_help_documents_the_derived_output_directory() {
    let directory = tempfile::tempdir().expect("tempdir");
    let output = rpmx_in(directory.path(), &["build", "--help"]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("packages/<arch>"), "{text}");
    assert!(text.contains("--out-dir"), "{text}");
    assert!(text.contains("default: package.yaml"), "{text}");
}
