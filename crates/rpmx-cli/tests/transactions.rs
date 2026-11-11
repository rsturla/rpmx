use std::path::{Path, PathBuf};
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

fn package(directory: &Path, version: &str) -> PathBuf {
    let stage = directory.join(format!("stage-{version}"));
    std::fs::create_dir_all(stage.join("etc")).unwrap();
    std::fs::create_dir_all(stage.join("usr/share/lifecycle-probe")).unwrap();
    std::fs::write(
        stage.join("etc/lifecycle-probe.conf"),
        format!("default-{version}\n"),
    )
    .unwrap();
    std::fs::write(stage.join("usr/share/lifecycle-probe/version"), version).unwrap();
    let recipe = directory.join(format!("probe-{version}.yaml"));
    std::fs::write(
        &recipe,
        format!(
            r#"schema: 1
package:
  name: lifecycle-probe
  version: "{version}"
  release: 1
  summary: Lifecycle probe
  license: MIT
  source-date-epoch: 1704067200
outputs:
  - name: lifecycle-probe
    files:
      config: [/etc/lifecycle-probe.conf]
      noreplace: [/etc/lifecycle-probe.conf]
    scriptlets:
      post: 'printf "%s\n" "$1" > /var/lib/lifecycle-probe.post'
      postun: 'printf "%s\n" "$1" > /var/lib/lifecycle-probe.postun'
"#
        ),
    )
    .unwrap();
    let output = directory.join(format!("out-{version}"));
    run(&[
        "build",
        recipe.to_str().unwrap(),
        "--stage",
        stage.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--no-tests",
    ]);
    output.join(std::env::consts::ARCH).join(format!(
        "lifecycle-probe-{version}-1.{}.rpm",
        std::env::consts::ARCH
    ))
}

#[test]
#[ignore = "set RPMX_TRANSACTION_TEST_ROOT and RPMX_TRANSACTION_TEST_LOCK to a prepared native RPM root"]
fn install_upgrade_config_and_erase_use_real_rpm_transactions() {
    let root = std::env::var("RPMX_TRANSACTION_TEST_ROOT").expect("test root");
    let lock = std::env::var("RPMX_TRANSACTION_TEST_LOCK").expect("test lock");
    let directory = tempfile::tempdir().unwrap();
    let old = package(directory.path(), "1");
    let current = package(directory.path(), "2");
    let files = directory.path().join("fixtures");
    std::fs::create_dir(&files).unwrap();
    std::fs::copy(old, files.join("previous.rpm")).unwrap();
    let recipe = directory.path().join("package.yaml");
    std::fs::write(
        &recipe,
        r#"schema: 1
package:
  name: lifecycle-probe
  version: "2"
  release: 1
  summary: Lifecycle probe
  license: MIT
outputs:
  - name: lifecycle-probe
tests:
  - name: lifecycle
    stage: transaction
    files: fixtures
    script: |
      if rpm -q lifecycle-probe; then exit 1; fi
      rpm -i "$TEST_FILES/previous.rpm"
      test "$(cat /var/lib/lifecycle-probe.post)" = 1
      printf 'user-value\n' > /etc/lifecycle-probe.conf
      rpm -U "$TEST_RPMS"/lifecycle-probe-2-*.rpm
      test "$(rpm -q --qf '%{VERSION}' lifecycle-probe)" = 2
      test "$(cat /etc/lifecycle-probe.conf)" = user-value
      test "$(cat /etc/lifecycle-probe.conf.rpmnew)" = default-2
      test "$(cat /usr/share/lifecycle-probe/version)" = 2
      test "$(cat /var/lib/lifecycle-probe.post)" = 2
      test "$(cat /var/lib/lifecycle-probe.postun)" = 1
      rpm -e lifecycle-probe
      test ! -e /usr/share/lifecycle-probe/version
      test "$(cat /var/lib/lifecycle-probe.postun)" = 0
      if rpm -q lifecycle-probe; then exit 1; fi
  - name: isolated
    stage: transaction
    script: |
      if rpm -q lifecycle-probe; then exit 1; fi
      test ! -e /var/lib/lifecycle-probe.post
      test ! -e /var/lib/lifecycle-probe.postun
"#,
    )
    .unwrap();
    run(&[
        "test",
        recipe.to_str().unwrap(),
        "--test-root",
        &root,
        "--test-root-lock",
        &lock,
        "--install-dir",
        current.parent().unwrap().to_str().unwrap(),
        "--test-output",
        directory.path().join("results").to_str().unwrap(),
        "--offline",
        "--json",
    ]);
    assert!(!Path::new(&root)
        .join("var/lib/lifecycle-probe.post")
        .exists());
}
