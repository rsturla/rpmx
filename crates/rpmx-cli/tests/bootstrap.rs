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

fn fixture(root: &Path) {
    write(
        &root.join("recipes/gcc/package.yaml"),
        r#"
schema: 2
package:
  name: gcc
  version: "1"
  release: 1
  summary: gcc
  license: MIT
outputs:
  - name: gcc
    metapackage: true
"#,
    );
    write(
        &root.join("recipes/glibc/package.yaml"),
        r#"
schema: 2
package:
  name: glibc
  version: "1"
  release: 1
  summary: glibc
  license: MIT
build:
  packages: [gcc]
outputs:
  - name: glibc
    metapackage: true
"#,
    );
    write(
        &root.join("stage0.profile.yaml"),
        r#"
document: rpmx-profile
version: 1
name: stage0
releasever: "44"
archs: [x86_64]
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
cross:
  bootstrap: true
  packages: [gcc]
"#,
    );
    write(
        &root.join("stage1.profile.yaml"),
        r#"
document: rpmx-profile
version: 1
name: stage1
releasever: "44"
archs: [x86_64]
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
"#,
    );
    write(
        &root.join("bootstrap.yaml"),
        r#"
document: rpmx-bootstrap
version: 1
name: demo
archs: [x86_64]
recipe-root: recipes
source-date-epoch: 1704067200
seed:
  name: fedora-44
  image: registry.fedoraproject.org/fedora:44
  releasever: "44"
  generation: "2026.01"
stages:
  - name: stage0
    profile: stage0.profile.yaml
    packages: [glibc, gcc]
    output: out/stage0
    seed: true
    extra-tools: [bison, python3]
  - name: stage1
    profile: stage1.profile.yaml
    packages: all
    output: out
"#,
    );
}

#[test]
fn plan_json_exposes_the_ladder_and_effective_timestamp() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let output = rpmx(&["bootstrap", "plan", manifest.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON document");
    assert_eq!(value["schema"], 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["command"], "bootstrap-plan");
    assert_eq!(value["document"], "rpmx-bootstrap-plan");
    assert_eq!(value["version"], 1);
    assert_eq!(value["seed"]["name"], "fedora-44");
    assert_eq!(value["seed"]["generation"], "2026.01");
    assert_eq!(value["source_date_epoch"], 1704067200);

    let stages = value["stages"].as_array().expect("stages");
    assert_eq!(stages.len(), 2);
    assert_eq!(stages[0]["name"], "stage0");
    assert_eq!(stages[0]["arch"], "x86_64");
    assert_eq!(stages[0]["inputs"], serde_json::json!([]));
    assert_eq!(stages[0]["seed"], true);
    assert_eq!(stages[0]["root"], "bootstrap/roots/stage0/x86_64");
    assert_eq!(stages[0]["lock"], "bootstrap/locks/stage0/x86_64.lock.yaml");
    assert_eq!(stages[0]["repo"], "out/stage0/x86_64");
    assert_eq!(stages[0]["repo_id"], "bootstrap-stage0");
    assert_eq!(stages[0]["order"], serde_json::json!(["gcc", "glibc"]));
    assert_eq!(stages[0]["source_date_epoch"], 1704067200);
    assert_eq!(stages[1]["inputs"], serde_json::json!(["stage0"]));
    assert_eq!(stages[1]["repo"], "out/x86_64");
}

#[test]
fn plan_human_output_names_the_seed_and_next_command() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let output = rpmx(&["bootstrap", "plan", manifest.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("seed   name=fedora-44"), "{text}");
    assert!(text.contains("generation=2026.01"), "{text}");
    assert!(text.contains("epoch  1704067200"), "{text}");
    assert!(
        text.contains("stage0  profile=stage0.profile.yaml"),
        "{text}"
    );
    assert!(text.contains("  repo   out/stage0/x86_64"), "{text}");
    assert!(text.contains("  tools  +bison +python3"), "{text}");
    assert!(text.contains("  order  gcc, glibc"), "{text}");
    assert!(text.contains("next: rpmx bootstrap build"), "{text}");
}

#[test]
fn a_bad_architecture_is_actionable_and_exits_2() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let output = rpmx(&[
        "bootstrap",
        "plan",
        manifest.to_str().unwrap(),
        "--arch",
        "aarch64",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["ok"], false);
    let diagnostic = &value["diagnostics"][0];
    assert_eq!(diagnostic["code"], "RPMX-BOOTSTRAP-009");
    assert!(
        diagnostic["message"].as_str().unwrap().contains("x86_64"),
        "{diagnostic}"
    );
}

#[test]
fn an_unknown_manifest_field_is_a_manifest_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    write(&manifest, &format!("{text}\nunexpected: true\n"));
    let output = rpmx(&["bootstrap", "plan", manifest.to_str().unwrap(), "--json"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["diagnostics"][0]["code"], "RPMX-BOOTSTRAP-001");
}

#[test]
fn an_unknown_stage_lists_the_available_stages() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let output = rpmx(&[
        "bootstrap",
        "build",
        manifest.to_str().unwrap(),
        "--stage",
        "nope",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["diagnostics"][0]["code"], "RPMX-BOOTSTRAP-009");
    assert!(
        value["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("stage0"),
        "{value}"
    );
}

#[test]
fn a_multi_arch_build_requires_an_arch() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("archs: [x86_64]", "archs: [x86_64, aarch64]");
    write(&manifest, &text);
    for file in ["stage0.profile.yaml", "stage1.profile.yaml"] {
        let path = directory.path().join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("archs: [x86_64]", "archs: [x86_64, aarch64]");
        write(&path, &text);
    }
    let output = rpmx(&["bootstrap", "build", manifest.to_str().unwrap(), "--json"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["diagnostics"][0]["code"], "RPMX-BOOTSTRAP-009");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("--arch"));
}

#[test]
fn multi_arch_plans_have_independent_roots_and_locks() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("archs: [x86_64]", "archs: [x86_64, aarch64]");
    write(&manifest, &text);
    for file in ["stage0.profile.yaml", "stage1.profile.yaml"] {
        let path = directory.path().join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("archs: [x86_64]", "archs: [x86_64, aarch64]");
        write(&path, &text);
    }

    let output = rpmx(&["bootstrap", "plan", manifest.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    let stages = value["stages"].as_array().expect("stages");
    assert_eq!(stages.len(), 4);
    assert_eq!(stages[0]["root"], "bootstrap/roots/stage0/x86_64");
    assert_eq!(stages[0]["lock"], "bootstrap/locks/stage0/x86_64.lock.yaml");
    assert_eq!(stages[2]["root"], "bootstrap/roots/stage0/aarch64");
    assert_eq!(
        stages[2]["lock"],
        "bootstrap/locks/stage0/aarch64.lock.yaml"
    );
}

#[test]
fn a_stage_without_its_input_repository_fails_before_resolving() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let output = rpmx(&[
        "bootstrap",
        "build",
        manifest.to_str().unwrap(),
        "--stage",
        "stage1",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["diagnostics"][0]["code"], "RPMX-BOOTSTRAP-007");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("stage0"));
}

#[test]
fn a_missing_locked_root_says_how_to_create_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let output = rpmx(&[
        "bootstrap",
        "build",
        manifest.to_str().unwrap(),
        "--locked",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    assert_eq!(value["diagnostics"][0]["code"], "RPMX-BOOTSTRAP-006");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("without --locked"));
}

#[test]
fn offline_without_a_cached_root_names_the_stage_and_online_command() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let output = rpmx(&[
        "bootstrap",
        "build",
        manifest.to_str().unwrap(),
        "--offline",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    let diagnostic = &value["diagnostics"][0];
    assert_eq!(diagnostic["code"], "RPMX-ROOT-016");
    assert!(diagnostic["message"].as_str().unwrap().contains("stage0"));
    assert!(diagnostic["message"]
        .as_str()
        .unwrap()
        .contains("--offline"));
}

#[test]
fn refresh_cannot_be_combined_with_locked() {
    let directory = tempfile::tempdir().expect("tempdir");
    fixture(directory.path());
    let manifest = directory.path().join("bootstrap.yaml");
    let output = rpmx(&[
        "bootstrap",
        "build",
        manifest.to_str().unwrap(),
        "--locked",
        "--refresh",
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn bootstrap_rebuild_requires_a_replacement_srpm_directory() {
    let output = rpmx(&["bootstrap", "rebuild", "probe.src.rpm"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--srpm-out"), "stderr: {stderr}");
}

#[test]
fn the_checked_in_example_ladder_plans() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .current_dir(&root)
        .args(["bootstrap", "plan", "examples/bootstrap/bootstrap.yaml"])
        .output()
        .expect("run rpmx");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let expected =
        std::fs::read_to_string(root.join("examples/bootstrap/plan.txt")).expect("checked-in plan");
    assert_eq!(text, expected);
}
