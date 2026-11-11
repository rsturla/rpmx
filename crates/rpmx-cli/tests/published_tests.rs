use std::path::Path;
use std::process::Command;

fn run(args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
#[ignore = "set RPMX_TEST_RESOLUTION_LOCK to a native root lock with a builder image and usable repositories"]
fn supplied_rpm_wins_over_an_identical_published_nevra() {
    let source_lock = std::env::var("RPMX_TEST_RESOLUTION_LOCK").expect("root lock");
    let directory = tempfile::tempdir().unwrap();
    let stage = directory.path().join("stage");
    std::fs::create_dir_all(stage.join("usr/share/resolution-probe")).unwrap();
    std::fs::write(
        stage.join("usr/share/resolution-probe/value"),
        "candidate\n",
    )
    .unwrap();
    let recipe = directory.path().join("package.yaml");
    std::fs::write(
        &recipe,
        r#"schema: 1
package:
  name: resolution-probe
  epoch: 2
  version: "1.0"
  release: 1
  summary: Test repository provenance
  license: MIT
  source-date-epoch: 1704067200
outputs:
  - name: resolution-probe
    tests:
      - name: installed-value
        stage: installed
        argv: [cat, /usr/share/resolution-probe/value]
        expect:
          stdout: "candidate\n"
"#,
    )
    .unwrap();
    let output = directory.path().join("out");
    run(&[
        "build",
        recipe.to_str().unwrap(),
        "--stage",
        stage.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--no-tests",
    ]);
    let binaries = output.join(std::env::consts::ARCH);
    let published = directory.path().join("published");
    run(&[
        "repo",
        "--rpms",
        binaries.to_str().unwrap(),
        "--output",
        published.to_str().unwrap(),
    ]);
    let mut lock = rpmx_core::root::load_lock(Path::new(&source_lock)).unwrap();
    lock.repositories.push(rpmx_core::profile::Repository {
        id: "published-probe".into(),
        baseurl: Some(format!("file://{}", published.display())),
        metalink: None,
        mirrorlist: None,
        gpgkey: None,
        repo_gpgcheck: false,
        exclude: Vec::new(),
        priority: None,
        auth: None,
    });
    lock.closure_digest = rpmx_core::root::closure_digest(
        &lock.releasever,
        &lock.archs,
        &lock.repositories,
        &lock.packages,
    );
    let test_lock = directory.path().join("root.lock.yaml");
    rpmx_core::root::write_lock(&test_lock, &lock).unwrap();
    run(&[
        "test",
        recipe.to_str().unwrap(),
        "--root-lock",
        test_lock.to_str().unwrap(),
        "--install-dir",
        binaries.to_str().unwrap(),
        "--test-output",
        directory.path().join("results").to_str().unwrap(),
        "--json",
    ]);
}
