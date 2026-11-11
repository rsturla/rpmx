use std::path::Path;
use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
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
    output
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
    let tested = run(&[
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
    let document: serde_json::Value = serde_json::from_slice(&tested.stdout).unwrap();
    assert_eq!(document["ok"], true);
    assert!(String::from_utf8_lossy(&tested.stderr)
        .contains("preparing test environment 1/1: output=resolution-probe; tools="));
    let evidence = directory.path().join("results/environments/1");
    let saved = rpmx_core::root::load_lock(&evidence.join("root.lock.yaml")).unwrap();
    assert!(saved
        .packages
        .values()
        .flatten()
        .any(|package| package.name == "bash"));
    assert!(!saved
        .repositories
        .iter()
        .any(|repository| repository.id == "test-under-test"));
    let environment: serde_json::Value =
        serde_json::from_slice(&std::fs::read(evidence.join("environment.json")).unwrap()).unwrap();
    assert_eq!(environment["status"], "ready");

    // Force an assembly failure after a valid pinned closure has been read.
    // Its evidence must survive even though no prepared root is returned.
    let mut mismatched = saved;
    mismatched.builder.as_mut().unwrap().image_digest = format!("sha256:{}", "0".repeat(64));
    let mismatch_lock = directory.path().join("mismatch.lock.yaml");
    rpmx_core::root::write_lock(&mismatch_lock, &mismatched).unwrap();
    let failed_results = directory.path().join("failed-results");
    let failed = Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args([
            "test",
            recipe.to_str().unwrap(),
            "--root-lock",
            test_lock.to_str().unwrap(),
            "--test-root-lock",
            mismatch_lock.to_str().unwrap(),
            "--install-dir",
            binaries.to_str().unwrap(),
            "--test-output",
            failed_results.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!failed.status.success(), "{failed:?}");
    let error: serde_json::Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(error["diagnostics"][0]["code"], "RPMX-ROOT-002");
    let failed_evidence = failed_results.join("environments/1");
    let failed_lock = rpmx_core::root::load_lock(&failed_evidence.join("root.lock.yaml")).unwrap();
    assert_eq!(failed_lock.closure_digest, mismatched.closure_digest);
    let failed_environment: serde_json::Value =
        serde_json::from_slice(&std::fs::read(failed_evidence.join("environment.json")).unwrap())
            .unwrap();
    assert_eq!(failed_environment["status"], "error");
    assert_eq!(failed_environment["error_code"], "RPMX-ROOT-002");
}
