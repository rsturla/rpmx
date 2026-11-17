use std::fs;
use std::process::Command;

use rpmx_core::package_spec::{BinaryPackageFile, BinaryPackageSpec};
use rpmx_rpm::sign::{generate_key, load_key, sign_package, KeygenOptions};
use rpmx_rpm::write_binary_rpm;

fn spec() -> BinaryPackageSpec {
    BinaryPackageSpec {
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
        files: vec![BinaryPackageFile {
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
        dependencies: rpmx_core::model::DependencySet {
            requires: Vec::new(),
            recommends: Vec::new(),
            suggests: Vec::new(),
            supplements: Vec::new(),
            enhances: Vec::new(),
            provides: Vec::new(),
            obsoletes: Vec::new(),
            conflicts: Vec::new(),
        },
        lint: Default::default(),
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

#[test]
fn generated_secret_keys_interoperate_with_gnupg() {
    use pgp::composed::{Deserializable, DetachedSignature, SignedPublicKey};
    use std::os::unix::fs::PermissionsExt;

    if !Command::new("gpg")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        eprintln!("gpg not installed; skipping GnuPG interoperability assertions");
        return;
    }
    for passphrase in [None, Some("test-only passphrase")] {
        let dir = tempfile::tempdir().expect("private keyring");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).expect("permissions");
        let generated = generate_key(KeygenOptions {
            user_id: "rpmx GnuPG test <rpmx@example.invalid>",
            bits: 2048,
            passphrase,
        })
        .expect("generate key");
        let secret = dir.path().join("secret.asc");
        fs::write(&secret, &generated.secret_armored).expect("secret key");
        let (public, _) =
            SignedPublicKey::from_string(&generated.public_armored).expect("public key");
        let gpg = |args: &[&std::ffi::OsStr]| {
            Command::new("gpg")
                .arg("--homedir")
                .arg(dir.path())
                .args([
                    "--no-options",
                    "--batch",
                    "--yes",
                    "--pinentry-mode",
                    "loopback",
                ])
                .arg("--passphrase")
                .arg(passphrase.unwrap_or(""))
                .args(args)
                .output()
                .map_err(|error| error.to_string())
        };
        // Always stop the private agent, including after a failed assertion below.
        let result = (|| -> Result<(), String> {
            let imported = gpg(&["--import".as_ref(), secret.as_os_str()])?;
            if !imported.status.success() {
                return Err(format!(
                    "GnuPG import failed: {}",
                    String::from_utf8_lossy(&imported.stderr)
                ));
            }
            for index in 0..16 {
                let data = format!("GnuPG RSA CRT interoperability message {index}\n");
                let payload = dir.path().join("message");
                let signature = dir.path().join("message.asc");
                fs::write(&payload, &data).map_err(|error| error.to_string())?;
                let signed = gpg(&[
                    "--armor".as_ref(),
                    "--detach-sign".as_ref(),
                    "--output".as_ref(),
                    signature.as_os_str(),
                    payload.as_os_str(),
                ])?;
                if !signed.status.success() {
                    return Err(format!(
                        "GnuPG signing failed: {}",
                        String::from_utf8_lossy(&signed.stderr)
                    ));
                }
                let armored = fs::read_to_string(&signature).map_err(|error| error.to_string())?;
                let (parsed, _) =
                    DetachedSignature::from_string(&armored).map_err(|error| error.to_string())?;
                parsed
                    .verify(&public.primary_key, data.as_bytes())
                    .map_err(|error| error.to_string())?;
                let verified = gpg(&[
                    "--verify".as_ref(),
                    signature.as_os_str(),
                    payload.as_os_str(),
                ])?;
                if !verified.status.success() {
                    return Err(format!(
                        "GnuPG verification failed: {}",
                        String::from_utf8_lossy(&verified.stderr)
                    ));
                }
                fs::write(&payload, "altered message\n").map_err(|error| error.to_string())?;
                let tampered = gpg(&[
                    "--verify".as_ref(),
                    signature.as_os_str(),
                    payload.as_os_str(),
                ])?;
                if tampered.status.success()
                    || parsed
                        .verify(&public.primary_key, b"altered message\n")
                        .is_ok()
                {
                    return Err("altered message accepted".into());
                }
            }
            Ok(())
        })();
        let _ = Command::new("gpgconf")
            .arg("--homedir")
            .arg(dir.path())
            .args(["--kill", "gpg-agent"])
            .status();
        result.expect("GnuPG must sign valid RSA signatures with generated keys");
    }
}
