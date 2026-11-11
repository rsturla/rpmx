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

fn bundle(directory: &Path) -> String {
    let root = directory.join("package.yaml");
    write(
        &root,
        r#"schema: 2
include:
  - sources.yaml
  - build.yaml
  - outputs.yaml
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
"#,
    );
    write(
        &directory.join("sources.yaml"),
        r#"sources:
  - name: probe-1.0.tar.gz
    kind: archive
    digest: "sha512:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    locators:
      - kind: https
        url: https://example.invalid/probe-1.0.tar.gz
"#,
    );
    write(
        &directory.join("build.yaml"),
        r#"build:
  packages: [bash]
  pipeline:
    - runs: "true"
"#,
    );
    write(
        &directory.join("outputs.yaml"),
        r#"outputs:
  - name: probe
    metapackage: true
"#,
    );
    root.display().to_string()
}

#[test]
fn a_split_recipe_lints_and_plans() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = bundle(directory.path());
    let lint = rpmx(&["lint", &root]);
    assert!(lint.status.success(), "{lint:?}");
    let plan = rpmx(&["plan", &root, "--json"]);
    assert!(plan.status.success(), "{plan:?}");
    let value: serde_json::Value = serde_json::from_slice(&plan.stdout).expect("plan JSON");
    assert_eq!(value["plan"]["cells"][0]["package"], "probe");
}

#[test]
fn a_diagnostic_names_the_file_that_owns_the_section() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = bundle(directory.path());
    write(
        &directory.path().join("build.yaml"),
        "build:\n  packages: [bash, bash]\n  pipeline:\n    - runs: \"true\"\n",
    );
    let output = rpmx(&["lint", &root]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("build.yaml: build.packages"), "{stdout}");
}

#[test]
fn a_duplicate_section_is_rejected_with_both_files() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = bundle(directory.path());
    write(
        &directory.path().join("second-build.yaml"),
        "build:\n  pipeline: []\n",
    );
    let text = std::fs::read_to_string(&root).expect("root");
    std::fs::write(
        &root,
        text.replace(
            "  - build.yaml\n",
            "  - build.yaml\n  - second-build.yaml\n",
        ),
    )
    .expect("write");
    let output = rpmx(&["lint", &root]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("build.yaml"), "{stderr}");
    assert!(stderr.contains("second-build.yaml"), "{stderr}");
}

#[test]
fn the_lock_records_every_recipe_file_and_detects_drift() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = bundle(directory.path());
    let lock = rpmx(&["lock", &root]);
    assert!(lock.status.success(), "{lock:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("package.lock.json")).unwrap())
            .expect("lock JSON");
    let files: Vec<_> = value["recipe_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(files.contains(&"package.yaml"), "{files:?}");
    assert!(files.contains(&"build.yaml"), "{files:?}");
    assert!(files.contains(&"sources.yaml"), "{files:?}");

    let check = rpmx(&["lock", &root, "--check"]);
    assert!(check.status.success(), "{check:?}");
    write(
        &directory.path().join("build.yaml"),
        "build:\n  packages: [bash, make]\n  pipeline:\n    - runs: \"true\"\n",
    );
    let check = rpmx(&["lock", &root, "--check"]);
    assert_eq!(check.status.code(), Some(3), "{check:?}");
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("recipe-files") || stderr.contains("recipe"),
        "{stderr}"
    );
}

#[test]
fn bump_edits_the_sections_owner_files() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = bundle(directory.path());
    let output = rpmx(&["bump", "1.1", &root, "--dry-run", "--no-fetch"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--- package.yaml"), "{stdout}");
    assert!(stdout.contains("--- sources.yaml"), "{stdout}");
    assert!(stdout.contains("version: \"1.1\""), "{stdout}");
    assert!(stdout.contains("probe-1.1.tar.gz"), "{stdout}");

    let write = rpmx(&["bump", "1.1", &root, "--no-fetch"]);
    assert!(write.status.success(), "{write:?}");
    let package = std::fs::read_to_string(directory.path().join("package.yaml")).unwrap();
    let sources = std::fs::read_to_string(directory.path().join("sources.yaml")).unwrap();
    assert!(package.contains("version: \"1.1\""), "{package}");
    assert!(sources.contains("probe-1.1.tar.gz"), "{sources}");
    let lint = rpmx(&["lint", &root]);
    assert!(lint.status.success(), "{lint:?}");
}
