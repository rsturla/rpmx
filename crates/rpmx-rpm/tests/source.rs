use std::fs;
use std::process::Command;

use rpmx_core::package_spec::{SourcePackageFile, SourcePackageSpec};
use rpmx_rpm::write_srpm;

fn spec() -> SourcePackageSpec {
    SourcePackageSpec {
        name: "demo".to_string(),
        version: "1.0".to_string(),
        release: "1".to_string(),
        epoch: None,
        arch: "x86_64".to_string(),
        summary: "demo source package".to_string(),
        license: "MIT".to_string(),
        description: "demo".to_string(),
        source_date_epoch: 1_700_000_000,
        files: vec![
            SourcePackageFile {
                path: "package.yaml".to_string(),
                data: b"schema: 2\n".to_vec(),
                mode: 0o644,
            },
            SourcePackageFile {
                path: "demo-1.0.tar.gz".to_string(),
                data: b"source bytes\n".to_vec(),
                mode: 0o644,
            },
        ],
    }
}

fn rpm_available() -> bool {
    Command::new("rpm")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn source_package_is_readable_by_stock_rpm() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("demo-1.0-1.src.rpm");
    write_srpm(&spec(), &path).expect("write SRPM");

    if !rpm_available() {
        eprintln!("rpm not installed; skipping stock-rpm assertions");
        return;
    }

    let query = Command::new("rpm")
        .args([
            "-qp",
            "--qf",
            "%{SOURCEPACKAGE} %{ARCH} %{PAYLOADFORMAT} %{PAYLOADCOMPRESSOR}\\n",
        ])
        .arg(&path)
        .output()
        .expect("run rpm");
    assert!(query.status.success(), "rpm -qp failed");
    assert_eq!(
        String::from_utf8_lossy(&query.stdout),
        "1 x86_64 cpio gzip\n"
    );

    let files = Command::new("rpm")
        .args(["-qpl"])
        .arg(&path)
        .output()
        .expect("run rpm -qpl");
    let files = String::from_utf8_lossy(&files.stdout);
    assert!(files.lines().any(|line| line == "package.yaml"));
    assert!(files.lines().any(|line| line == "demo-1.0.tar.gz"));
    assert!(!files.lines().any(|line| line.starts_with('/')));
}

#[test]
fn source_package_is_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.src.rpm");
    let second = dir.path().join("second.src.rpm");
    write_srpm(&spec(), &first).expect("first");
    write_srpm(&spec(), &second).expect("second");
    assert_eq!(
        fs::read(&first).expect("read first"),
        fs::read(&second).expect("read second")
    );
}
