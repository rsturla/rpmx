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

const EXPLAIN_RECIPE: &str = r#"
schema: 1
package:
  name: explain-components
  version: "1.0"
  release: 1
  summary: Explain component inventory
  license: MIT
  source-date-epoch: 1700000000
outputs:
  - name: explain-components
    files:
      include: ["/usr/bin/app"]
"#;

#[test]
fn explain_components_shows_discovery_evidence() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    fs::write(&recipe, EXPLAIN_RECIPE).expect("recipe");
    let source = directory.path().join("source");
    fs::create_dir_all(source.join("vendor")).expect("vendor");
    fs::write(
        source.join("vendor/modules.txt"),
        "# github.com/spf13/cobra v1.8.1\n",
    )
    .expect("modules");

    let output = rpmx(&[
        "explain",
        "components",
        "--source",
        source.to_str().unwrap(),
        "--json",
        recipe.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["ok"], true);
    assert_eq!(value["source"], source.to_str().unwrap());
    let output = &value["outputs"][0];
    assert_eq!(output["name"], "explain-components");
    assert_eq!(
        output["provides"],
        serde_json::json!(["bundled(golang(github.com/spf13/cobra)) = 1.8.1"])
    );
    assert_eq!(
        output["components"][0]["evidence"][0]["path"],
        "vendor/modules.txt"
    );
    assert_eq!(output["components"][0]["scope"], "vendored");
}

#[test]
fn explain_components_without_source_shows_recipe_declarations() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    fs::write(
        &recipe,
        r#"schema: 1
package:
  name: explain-declared
  version: "1.0"
  release: 1
  summary: Explain declared components
  license: MIT
  source-date-epoch: 1700000000
outputs:
  - name: explain-declared
    files:
      include: ["/usr/bin/app"]
    bundled:
      - name: zlib
        version: "1.3.1"
"#,
    )
    .expect("recipe");
    let output = rpmx(&["explain", "components", "--json", recipe.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["source"], serde_json::Value::Null);
    assert_eq!(
        value["outputs"][0]["provides"],
        serde_json::json!(["bundled(zlib) = 1.3.1"])
    );
    assert_eq!(value["outputs"][0]["components"][0]["source"], "recipe");
}

#[test]
fn explain_components_applies_generator_exclusions() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    fs::write(
        &recipe,
        r#"schema: 1
package:
  name: explain-excluded
  version: "1.0"
  release: 1
  summary: Explain excluded components
  license: MIT
  source-date-epoch: 1700000000
outputs:
  - name: explain-excluded
    files:
      include: ["/usr/bin/app"]
    bundled:
      - name: zlib
        version: "1.3.1"
    generators:
      exclude:
        - "bundled(zlib) = 1.3.1"
"#,
    )
    .expect("recipe");
    let output = rpmx(&["explain", "components", "--json", recipe.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["outputs"][0]["provides"], serde_json::json!([]));
    assert_eq!(
        value["outputs"][0]["components"][0]["provide"],
        "bundled(zlib) = 1.3.1"
    );
}

#[test]
fn explain_components_rejects_a_non_directory_source() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    fs::write(&recipe, EXPLAIN_RECIPE).expect("recipe");
    let source = directory.path().join("not-a-directory");
    fs::write(&source, "file").expect("file");
    let output = rpmx(&[
        "explain",
        "components",
        "--source",
        source.to_str().unwrap(),
        "--json",
        recipe.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["diagnostics"][0]["code"], "RPMX-CLI-007");
}

#[test]
fn explain_components_shows_components_when_the_generator_is_disabled() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    fs::write(
        &recipe,
        r#"schema: 1
package:
  name: explain-disabled
  version: "1.0"
  release: 1
  summary: Explain disabled components
  license: MIT
  source-date-epoch: 1700000000
outputs:
  - name: explain-disabled
    files:
      include: ["/usr/bin/app"]
    bundled:
      - name: zlib
        version: "1.3.1"
    generators:
      bundled: false
"#,
    )
    .expect("recipe");
    let output = rpmx(&["explain", "components", "--json", recipe.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["outputs"][0]["provides"], serde_json::json!([]));
    assert_eq!(value["outputs"][0]["bundled"], false);
    assert_eq!(value["outputs"][0]["components"][0]["name"], "zlib");
}

#[test]
fn bundled_declare_step_writes_a_version_from_a_file() {
    if Command::new("bwrap").arg("--version").output().is_err() {
        eprintln!("bubblewrap is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = directory.path().join("src");
    fs::create_dir_all(source.join("third_party/zlib")).expect("source");
    fs::write(source.join("third_party/zlib/VERSION"), "1.3.1\n").expect("version");
    let recipe = directory.path().join("package.yaml");
    fs::write(
        &recipe,
        r#"schema: 1
package:
  name: bundled-declare
  version: "1.0"
  release: 1
  summary: Declare a bundled component with the helper step
  license: MIT
  source-date-epoch: 1700000000
context: src
source-assembly:
  pipeline:
    - uses: bundled/declare
      with:
        output: bundled-declare
        name: zlib
        version-file: third_party/zlib/VERSION
outputs:
  - name: bundled-declare
    files:
      include: ["/usr/bin/app"]
"#,
    )
    .expect("recipe");
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
    let component = inventory["components"]
        .as_array()
        .expect("components")
        .iter()
        .find(|component| component["name"] == "zlib")
        .expect("zlib component");
    assert_eq!(component["version"], "1.3.1");
    assert_eq!(component["source"], "build");
}

#[test]
fn bundled_declare_rejects_version_and_version_file_together() {
    if Command::new("bwrap").arg("--version").output().is_err() {
        eprintln!("bubblewrap is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let source = directory.path().join("src");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("VERSION"), "1.3.1\n").expect("version");
    let recipe = directory.path().join("package.yaml");
    fs::write(
        &recipe,
        r#"schema: 1
package:
  name: bundled-declare-conflict
  version: "1.0"
  release: 1
  summary: Reject conflicting declaration inputs
  license: MIT
  source-date-epoch: 1700000000
context: src
source-assembly:
  pipeline:
    - uses: bundled/declare
      with:
        output: bundled-declare-conflict
        name: zlib
        version: "1.3.1"
        version-file: VERSION
outputs:
  - name: bundled-declare-conflict
    files:
      include: ["/usr/bin/app"]
"#,
    )
    .expect("recipe");
    let out = directory.path().join("source");
    let output = rpmx(&[
        "source",
        "--unsafe-host-root",
        "--output",
        out.to_str().unwrap(),
        recipe.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("set version or version-file, not both"),
        "{stderr}"
    );
}

#[test]
fn bundled_declare_supports_unversioned_components() {
    if Command::new("bwrap").arg("--version").output().is_err() {
        eprintln!("bubblewrap is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    fs::write(
        &recipe,
        r#"schema: 1
package:
  name: bundled-unversioned
  version: "1.0"
  release: 1
  summary: Declare an unversioned component
  license: MIT
  source-date-epoch: 1700000000
source-assembly:
  pipeline:
    - uses: bundled/declare
      with:
        output: bundled-unversioned
        name: zlib
outputs:
  - name: bundled-unversioned
    files:
      include: ["/usr/bin/app"]
"#,
    )
    .expect("recipe");
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
    let component = inventory["components"]
        .as_array()
        .expect("components")
        .iter()
        .find(|component| component["name"] == "zlib")
        .expect("zlib component");
    assert!(component.get("version").is_none(), "{component}");
}

#[test]
fn bundled_declare_rejects_an_invalid_name() {
    if Command::new("bwrap").arg("--version").output().is_err() {
        eprintln!("bubblewrap is not installed; skipping");
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    fs::write(
        &recipe,
        r#"schema: 1
package:
  name: bundled-invalid
  version: "1.0"
  release: 1
  summary: Reject an invalid bundled name
  license: MIT
  source-date-epoch: 1700000000
source-assembly:
  pipeline:
    - uses: bundled/declare
      with:
        output: bundled-invalid
        name: "bad;name"
outputs:
  - name: bundled-invalid
    files:
      include: ["/usr/bin/app"]
"#,
    )
    .expect("recipe");
    let out = directory.path().join("source");
    let output = rpmx(&[
        "source",
        "--unsafe-host-root",
        "--output",
        out.to_str().unwrap(),
        recipe.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid name"), "{stderr}");
}
