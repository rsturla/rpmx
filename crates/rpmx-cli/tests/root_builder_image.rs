use std::path::Path;
use std::process::{Command, Output};

fn invoke(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .current_dir(directory)
        .args(arguments)
        .output()
        .expect("run rpmx")
}

fn run(directory: &Path, arguments: &[&str]) {
    let output = invoke(directory, arguments);
    assert!(
        output.status.success(),
        "{arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires rootless Podman and access to the Fedora image registry"]
fn root_assembly_uses_the_locked_image_when_the_original_tag_disappears() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path();
    std::fs::create_dir(directory.join("stage")).unwrap();
    std::fs::write(
        directory.join("package.yaml"),
        r#"
schema: 1
package:
  name: builder-image-probe
  version: "1"
  release: 1
  summary: Locked builder image regression
  license: MIT
  source-date-epoch: 1704067200
outputs:
  - name: builder-image-probe
    metapackage: true
"#,
    )
    .unwrap();
    run(
        directory,
        &[
            "build",
            "package.yaml",
            "--stage",
            "stage",
            "--output",
            "rpms",
            "--no-tests",
        ],
    );
    let architecture = std::env::consts::ARCH;
    let repository = directory.join("rpms").join(architecture);
    run(
        directory,
        &[
            "repo",
            "--rpms",
            repository.to_str().unwrap(),
            "--output",
            "repository",
        ],
    );
    let profile = serde_json::json!({
        "document": "rpmx-profile", "version": 1, "name": "locked-builder",
        "archs": [architecture], "releasever": "44",
        "builder": {"image": "registry.fedoraproject.org/fedora:44"},
        "repositories": [{"id": "fixture", "baseurl": format!("file://{}", directory.join("repository").display())}],
        "packages": {"install": ["builder-image-probe"]},
    });
    std::fs::write(
        directory.join("profile.yaml"),
        serde_yaml_ng::to_string(&profile).unwrap(),
    )
    .unwrap();
    run(
        directory,
        &[
            "root",
            "resolve",
            "profile.yaml",
            "--output",
            "root.lock.yaml",
            "--cache",
            "cache",
        ],
    );
    let mut lock = rpmx_core::root::load_lock(&directory.join("root.lock.yaml")).unwrap();
    let original_digest = lock.builder.as_ref().unwrap().image_digest.clone();
    lock.builder.as_mut().unwrap().image =
        "registry.fedoraproject.org/fedora:rpmx-deliberately-absent".to_string();
    rpmx_core::root::write_lock(&directory.join("pinned.lock.yaml"), &lock).unwrap();
    run(
        directory,
        &[
            "root",
            "assemble",
            "pinned.lock.yaml",
            "--root",
            "installed",
            "--output",
            "assembled.lock.yaml",
            "--cache",
            "cache",
        ],
    );
    let assembled = rpmx_core::root::load_lock(&directory.join("assembled.lock.yaml")).unwrap();
    assert_eq!(
        assembled.builder.as_ref().unwrap().image_digest,
        original_digest
    );
    assert!(assembled.root_digest.is_some());
    assert!(assembled
        .packages
        .values()
        .flatten()
        .any(|package| package.name == "builder-image-probe"));
}
