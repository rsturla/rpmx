use std::process::Command;

const RECIPE: &str = r#"
schema: 2
package:
  name: probe
  version: "1.0"
  release: 1
  summary: probe
  license: MIT
outputs:
  - name: probe
    files:
      include: ["/usr/bin/probe"]
"#;

fn rpmx() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
}

#[test]
fn rebuild_uses_the_policy_recorded_in_the_srpm() {
    let work = tempfile::tempdir().expect("tempdir");
    let recipe = work.path().join("package.yaml");
    let sources = work.path().join("sources");
    let output = work.path().join("out");
    std::fs::write(&recipe, RECIPE).expect("recipe");
    std::fs::create_dir(&sources).expect("sources");
    std::fs::create_dir(&output).expect("output");

    let srpm = rpmx()
        .arg("srpm")
        .arg(&recipe)
        .arg("--sources")
        .arg(&sources)
        .arg("--output")
        .arg(&output)
        .arg("--dist-tag")
        .arg("fc44")
        .arg("--build-release")
        .arg("7")
        .output()
        .expect("run srpm");
    assert!(
        srpm.status.success(),
        "srpm failed: {}",
        String::from_utf8_lossy(&srpm.stderr)
    );
    let srpm = output.join("probe-1.0-1.fc44+build7.src.rpm");
    assert!(srpm.is_file(), "missing {}", srpm.display());

    let mismatch = rpmx()
        .arg("rebuild")
        .arg("--output")
        .arg(work.path().join("rebuilt"))
        .arg(&srpm)
        .arg("--dist-tag")
        .arg("fc45")
        .output()
        .expect("run mismatching rebuild");
    // A rebuild that contradicts the SRPM lock is lock drift.
    assert_eq!(mismatch.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&mismatch.stderr);
    assert!(stderr.contains("RPMX-SRPM-012"), "stderr: {stderr}");
    assert!(
        stderr.contains("does not match the lock"),
        "stderr: {stderr}"
    );

    // Matching (or omitted) flags must get past the policy and NVR checks.
    // This build has no root, so it stops at the root requirement instead.
    let matching = rpmx()
        .arg("rebuild")
        .arg("--output")
        .arg(work.path().join("rebuilt"))
        .arg(&srpm)
        .output()
        .expect("run matching rebuild");
    // A missing root is a resolution failure.
    assert_eq!(matching.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&matching.stderr);
    assert!(!stderr.contains("RPMX-SRPM-012"), "stderr: {stderr}");
    assert!(stderr.contains("RPMX-ROOT-016"), "stderr: {stderr}");
}
