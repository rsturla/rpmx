use std::fs;

use rpmx_core::package_spec::{SourcePackageFile, SourcePackageSpec};
use rpmx_rpm::extract::extract_srpm;
use rpmx_rpm::write_srpm;

fn spec() -> SourcePackageSpec {
    SourcePackageSpec {
        name: "extract-demo".to_string(),
        version: "1.0".to_string(),
        release: "1".to_string(),
        epoch: None,
        arch: "x86_64".to_string(),
        summary: "extract demo".to_string(),
        license: "MIT".to_string(),
        description: "extract demo".to_string(),
        source_date_epoch: 1_700_000_000,
        files: vec![
            SourcePackageFile {
                path: "package.yaml".to_string(),
                data: b"schema: 2\n".to_vec(),
                mode: 0o644,
            },
            SourcePackageFile {
                path: "demo-1.0.tar.gz".to_string(),
                data: b"source\n".to_vec(),
                mode: 0o644,
            },
            SourcePackageFile {
                path: "package.lock.json".to_string(),
                data: br#"{"schema":1,"steps":[{"name":"go/build","digest":"sha512:abc"}]}"#
                    .to_vec(),
                mode: 0o644,
            },
            SourcePackageFile {
                path: "RPMX-MANIFEST.json".to_string(),
                data: br#"{"schema":1,"source_date_epoch":1700000000}"#.to_vec(),
                mode: 0o644,
            },
            SourcePackageFile {
                path: "RPMX-STEPS.json".to_string(),
                data: br#"{"acme/strip":"name: acme/strip\n"}"#.to_vec(),
                mode: 0o644,
            },
        ],
    }
}

#[test]
fn extracts_payload_steps_and_epoch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let srpm = dir.path().join("extract-demo-1.0-1.src.rpm");
    write_srpm(&spec(), &srpm).expect("write SRPM");

    let dest = dir.path().join("extract");
    let extracted = extract_srpm(&srpm, &dest).expect("extract");

    assert!(extracted.files.iter().any(|file| file == "package.yaml"));
    assert!(extracted.files.iter().any(|file| file == "demo-1.0.tar.gz"));
    assert_eq!(extracted.source_date_epoch, 1_700_000_000);
    assert_eq!(
        extracted.steps.get("acme/strip").map(String::as_str),
        Some("name: acme/strip\n")
    );
    assert_eq!(
        extracted.lock_steps.get("go/build").map(String::as_str),
        Some("sha512:abc")
    );
    assert_eq!(
        fs::read(dest.join("demo-1.0.tar.gz")).expect("source"),
        b"source\n"
    );
}
