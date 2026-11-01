use std::process::{Command, Output};

fn rpmx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .expect("run rpmx")
}

const RECIPE: &str = r#"
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
source-assembly:
  pipeline:
    - runs: "echo assembled > README"
build:
  pipeline:
    - runs: "true"
outputs:
  - name: probe
    metapackage: true
"#;

#[test]
fn srpm_assembles_its_own_source_snapshot() {
    if Command::new("bwrap").arg("--version").output().is_err() {
        eprintln!("bubblewrap is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    std::fs::write(&recipe, RECIPE).expect("recipe");
    let out = directory.path().join("out");
    let output = rpmx(&[
        "srpm",
        recipe.to_str().unwrap(),
        "--unsafe-host-root",
        "--output",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    let found = std::fs::read_dir(&out)
        .expect("output directory")
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().ends_with(".src.rpm"));
    assert!(found, "no source RPM written to {}", out.display());
}
