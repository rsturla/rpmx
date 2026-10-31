use std::fs;
use std::process::Command;

use rpmx_rpm::sign::{generate_key, load_key, sign_package, KeygenOptions};
use rpmx_rpm::{write_binary_rpm, BinaryFile, BinarySpec};

fn spec() -> BinarySpec {
    BinarySpec {
        name: "signed-demo".to_string(),
        version: "1.0".to_string(),
        release: "1".to_string(),
        epoch: None,
        arch: "x86_64".to_string(),
        summary: "signed demo".to_string(),
        license: "MIT".to_string(),
        description: "signed demo".to_string(),
        url: None,
        source_rpm: "signed-demo-1.0-1.src.rpm".to_string(),
        source_date_epoch: 1_700_000_000,
        files: vec![BinaryFile {
            path: "/usr/share/signed-demo".to_string(),
            data: b"hello\n".to_vec(),
            mode: 0o644,
            target: None,
            user: None,
            group: None,
            caps: None,
            config: false,
            noreplace: false,
            doc: false,
            license: false,
        }],
        directories: Vec::new(),
        requires: Vec::new(),
        provides: Vec::new(),
        obsoletes: Vec::new(),
        conflicts: Vec::new(),
        scriptlets: Default::default(),
    }
}

fn rpm_available() -> bool {
    Command::new("rpm")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn signed_package_verifies_with_stock_rpm() {
    if !rpm_available() {
        eprintln!("rpm not installed; skipping stock-rpm assertions");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let key_path = dir.path().join("test.asc");
    let public_path = dir.path().join("test.asc.pub");
    let generated = generate_key(KeygenOptions {
        user_id: "rpmx test <rpmx@example.invalid>",
        bits: 2048,
        passphrase: None,
    })
    .expect("generate key");
    fs::write(&key_path, &generated.secret_armored).expect("write secret key");
    fs::write(&public_path, &generated.public_armored).expect("write public key");

    let rpm_path = dir.path().join("signed-demo-1.0-1.x86_64.rpm");
    write_binary_rpm(&spec(), &rpm_path).expect("write RPM");

    let key = load_key(&key_path, None).expect("load key");
    assert_eq!(key.fingerprint(), generated.fingerprint);
    sign_package(&rpm_path, &key, None).expect("sign");

    let dbpath = dir.path().join("rpmdb");
    fs::create_dir(&dbpath).expect("rpmdb");
    let import = Command::new("rpm")
        .args(["--dbpath", dbpath.to_str().unwrap(), "--import"])
        .arg(&public_path)
        .output()
        .expect("rpm --import");
    assert!(import.status.success(), "import failed: {import:?}");

    let check = Command::new("rpm")
        .args(["--dbpath", dbpath.to_str().unwrap(), "--checksig"])
        .arg(&rpm_path)
        .output()
        .expect("rpm --checksig");
    let report = String::from_utf8_lossy(&check.stdout);
    let errors = String::from_utf8_lossy(&check.stderr);
    assert!(
        check.status.success(),
        "checksig failed: {report}\n{errors}"
    );
    assert!(
        report.contains("signatures OK") || errors.contains("signatures OK"),
        "no valid signature reported: {report}\n{errors}"
    );
}

#[test]
fn signature_before_the_key_creation_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let key_path = dir.path().join("test.asc");
    let generated = generate_key(KeygenOptions {
        user_id: "rpmx test <rpmx@example.invalid>",
        bits: 2048,
        passphrase: None,
    })
    .expect("generate key");
    fs::write(&key_path, &generated.secret_armored).expect("write secret key");

    let rpm_path = dir.path().join("signed-demo-1.0-1.x86_64.rpm");
    write_binary_rpm(&spec(), &rpm_path).expect("write RPM");
    let key = load_key(&key_path, None).expect("load key");

    let error = sign_package(&rpm_path, &key, Some(1_600_000_000))
        .expect_err("a signature predating the key must be refused");
    assert_eq!(error.code, rpmx_core::error::codes::SIGN_004);
    assert!(error.message.contains("before signing key"), "{error}");
}
