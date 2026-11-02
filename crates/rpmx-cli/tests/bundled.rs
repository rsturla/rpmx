use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn rpmx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .expect("run rpmx")
}

fn rpm_available() -> bool {
    Command::new("rpm")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn find_rpm(directory: &Path) -> PathBuf {
    let mut rpms = Vec::new();
    for entry in fs::read_dir(directory).expect("output directory") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("rpm") {
            rpms.push(path);
        }
    }
    assert_eq!(rpms.len(), 1, "{rpms:?}");
    rpms.remove(0)
}

const STAGE_RECIPE: &str = r#"
schema: 1
package:
  name: bundled-stage
  version: "1.0"
  release: 1
  summary: A staged package with explicit bundled components
  license: MIT
  source-date-epoch: 1700000000
outputs:
  - name: bundled-stage
    files:
      include: ["/usr/bin/demo"]
    bundled:
      - name: zlib
        version: "1.3.1"
      - name: libpng
        version: "1.6.43"
"#;

#[test]
fn build_stage_emits_explicit_bundled_provides() {
    if !rpm_available() {
        eprintln!("rpm is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let stage = directory.path().join("stage");
    fs::create_dir_all(stage.join("usr/bin")).expect("stage");
    fs::write(stage.join("usr/bin/demo"), b"#!/bin/sh\necho demo\n").expect("binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            stage.join("usr/bin/demo"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("mode");
    }
    let recipe = directory.path().join("package.yaml");
    fs::write(&recipe, STAGE_RECIPE).expect("recipe");
    let out = directory.path().join("out");

    let output = rpmx(&[
        "build",
        "--stage",
        stage.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
        "--no-index",
        "--no-lint",
        recipe.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");

    let rpm = find_rpm(&out.join("x86_64"));
    let query = Command::new("rpm")
        .args(["-qp", "--provides"])
        .arg(&rpm)
        .output()
        .expect("rpm -qp");
    assert!(query.status.success(), "{query:?}");
    let provides = String::from_utf8_lossy(&query.stdout);
    assert!(provides.contains("bundled(zlib) = 1.3.1"), "{provides}");
    assert!(provides.contains("bundled(libpng) = 1.6.43"), "{provides}");
}

const SOURCE_RECIPE: &str = r#"
schema: 1
package:
  name: bundled-source
  version: "1.0"
  release: 1
  summary: A source tree with vendor metadata
  license: MIT
  source-date-epoch: 1700000000
context: src
outputs:
  - name: bundled-source
    files:
      include: ["/usr/bin/app"]
"#;

#[test]
fn source_assembly_writes_a_component_inventory() {
    if Command::new("bwrap").arg("--version").output().is_err() {
        eprintln!("bubblewrap is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = directory.path().join("src");
    fs::create_dir_all(source.join("vendor")).expect("vendor");
    fs::write(
        source.join("vendor/modules.txt"),
        "# github.com/spf13/cobra v1.8.1\n",
    )
    .expect("modules");
    fs::write(
        source.join("Cargo.lock"),
        r#"version = 4

[[package]]
name = "serde"
version = "1.0.203"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    )
    .expect("cargo lock");
    fs::create_dir_all(source.join(".rpmx/bundled/bundled-source")).expect("bundled dir");
    fs::write(
        source.join(".rpmx/bundled/bundled-source/zlib.list"),
        "zlib=1.3.1\n",
    )
    .expect("manual list");
    let recipe = directory.path().join("package.yaml");
    fs::write(&recipe, SOURCE_RECIPE).expect("recipe");
    let out = directory.path().join("source");

    let output = rpmx(&[
        "source",
        "--unsafe-host-root",
        "--output",
        out.to_str().unwrap(),
        recipe.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");

    let inventory: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(out.join("RPMX-BUNDLED.json")).expect("inventory"),
    )
    .expect("valid JSON");
    let components = inventory["components"].as_array().expect("components");
    assert!(components
        .iter()
        .any(|component| component["name"] == "github.com/spf13/cobra"));
    assert!(components
        .iter()
        .any(|component| component["name"] == "serde"));
    assert!(components
        .iter()
        .any(|component| component["name"] == "zlib"));

    let srpm_out = directory.path().join("srpm");
    let output = rpmx(&[
        "srpm",
        "--sources",
        out.to_str().unwrap(),
        "--output",
        srpm_out.to_str().unwrap(),
        recipe.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
}
