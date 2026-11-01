use std::process::{Command, Output};

const RECIPE: &str = r#"
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
sources:
  - name: probe.tar.gz
    kind: archive
    digest: "sha512:00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    locators:
      - kind: https
        url: https://example.invalid/probe-1.0.tar.gz
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
fn bump_updates_the_version_and_source_urls() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    std::fs::write(&recipe, RECIPE).expect("recipe");
    let output = rpmx(&["bump", "1.1", recipe.to_str().unwrap(), "--no-fetch"]);
    assert!(output.status.success(), "{output:?}");
    let text = std::fs::read_to_string(&recipe).expect("read");
    assert!(text.contains("version: \"1.1\""), "{text}");
    assert!(text.contains("probe-1.1.tar.gz"), "{text}");
}

#[test]
fn bump_dry_run_does_not_write() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    std::fs::write(&recipe, RECIPE).expect("recipe");
    let output = rpmx(&[
        "bump",
        "1.1",
        recipe.to_str().unwrap(),
        "--no-fetch",
        "--dry-run",
    ]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("+  version: \"1.1\""), "{text}");
    assert!(std::fs::read_to_string(&recipe).unwrap().contains("1.0"));
}
