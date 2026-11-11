use std::path::Path;
use std::process::{Command, Output};

fn rpmx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .unwrap()
}

fn recipe(directory: &Path, tests: &str) -> std::path::PathBuf {
    let path = directory.join("package.yaml");
    std::fs::write(
        &path,
        format!(
            r#"
schema: 2
package:
  name: fixture-probe
  version: "1"
  release: 1
  summary: Fixture probe
  license: MIT
  source-date-epoch: 1704067200
build:
  pipeline: []
outputs:
  - name: fixture-probe
    metapackage: true
tests:
{tests}
"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn fixtures_survive_the_srpm_and_change_the_recipe_digest() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("tests/include")).unwrap();
    let executable = directory.path().join("tests/driver.sh");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let fixture = directory.path().join("tests/include/probe.h");
    std::fs::write(&fixture, "#define ANSWER 42\n").unwrap();
    let path = recipe(
        directory.path(),
        "  - name: headers\n    files: tests\n    argv: ['true']",
    );
    let before = rpmx_core::load_recipe_bundle(&path).unwrap();
    std::fs::create_dir(directory.path().join("sources")).unwrap();
    let output = rpmx(&[
        "srpm",
        path.to_str().unwrap(),
        "--sources",
        directory.path().join("sources").to_str().unwrap(),
        "--output",
        directory.path().join("out").to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    let srpm = std::fs::read_dir(directory.path().join("out"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".src.rpm"))
        .unwrap();
    let extracted = directory.path().join("extracted");
    rpmx_rpm::extract::extract_srpm(&srpm, &extracted).unwrap();
    assert_eq!(
        std::fs::read(extracted.join("tests/include/probe.h")).unwrap(),
        std::fs::read(&fixture).unwrap()
    );
    let after = rpmx_core::load_recipe_bundle(extracted.join("package.yaml")).unwrap();
    assert_eq!(before.digest(), after.digest());
    assert_eq!(
        std::fs::metadata(extracted.join("tests/driver.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o755
    );
    std::fs::write(fixture, "#define ANSWER 0\n").unwrap();
    assert_ne!(
        before.digest(),
        rpmx_core::load_recipe_bundle(path).unwrap().digest()
    );
}
