use std::fs;
use std::process::Command;

use rpmx_core::package_spec::{BinaryPackageDirectory, BinaryPackageFile, BinaryPackageSpec};
use rpmx_rpm::write_binary_rpm;

fn spec() -> BinaryPackageSpec {
    BinaryPackageSpec {
        name: "demo-bin".to_string(),
        version: "1.0".to_string(),
        release: "1".to_string(),
        epoch: None,
        arch: "x86_64".to_string(),
        summary: "demo binary".to_string(),
        license: "MIT".to_string(),
        description: "demo".to_string(),
        url: Some("https://example.org".to_string()),
        source_rpm: "demo-bin-1.0-1.src.rpm".to_string(),
        source_date_epoch: 1_700_000_000,
        files: vec![
            BinaryPackageFile {
                path: "/usr/bin/demo-bin".to_string(),
                data: b"#!/bin/sh\necho demo\n".to_vec(),
                mode: 0o755,
                target: None,
                user: Some("demo".to_string()),
                group: Some("demo".to_string()),
                caps: Some("cap_net_raw=ep".to_string()),
                config: false,
                noreplace: false,
                doc: false,
                license: false,
            },
            BinaryPackageFile {
                path: "/usr/bin/demo-alias".to_string(),
                data: Vec::new(),
                mode: 0o777,
                target: Some("demo-bin".to_string()),
                user: None,
                group: None,
                caps: None,
                config: false,
                noreplace: false,
                doc: false,
                license: false,
            },
            BinaryPackageFile {
                path: "/etc/demo.conf".to_string(),
                data: b"setting=1\n".to_vec(),
                mode: 0o644,
                target: None,
                user: None,
                group: None,
                caps: None,
                config: true,
                noreplace: false,
                doc: false,
                license: false,
            },
        ],
        directories: vec![BinaryPackageDirectory {
            path: "/var/lib/demo".to_string(),
            mode: 0o750,
            user: Some("demo".to_string()),
            group: Some("demo".to_string()),
        }],
        requires: vec!["/bin/sh".to_string(), "glibc >= 2.39".to_string()],
        recommends: vec!["demo-extra >= 1.0".to_string()],
        suggests: vec!["demo-docs".to_string()],
        supplements: vec!["demo-plugin".to_string()],
        enhances: vec!["demo-host < 2.0".to_string()],
        provides: vec!["demo".to_string()],
        obsoletes: vec!["old-demo < 1.0".to_string()],
        conflicts: vec!["demo-server".to_string()],
        scriptlets: rpmx_core::model::Scriptlets {
            post: Some("echo installed\n".to_string()),
            ..Default::default()
        },
    }
}

fn rpm_available() -> bool {
    Command::new("rpm")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn binary_package_is_readable_by_stock_rpm() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("demo-bin-1.0-1.x86_64.rpm");
    write_binary_rpm(&spec(), &path).expect("write RPM");

    if !rpm_available() {
        eprintln!("rpm not installed; skipping stock-rpm assertions");
        return;
    }

    let query = Command::new("rpm")
        .args([
            "-qp",
            "--qf",
            "%{NAME} %{ARCH} %{SOURCERPM} %{PAYLOADCOMPRESSOR}\\n",
        ])
        .arg(&path)
        .output()
        .expect("run rpm");
    assert!(query.status.success(), "rpm -qp failed");
    assert_eq!(
        String::from_utf8_lossy(&query.stdout),
        "demo-bin x86_64 demo-bin-1.0-1.src.rpm gzip\n"
    );

    let files = Command::new("rpm")
        .args(["-qpl"])
        .arg(&path)
        .output()
        .expect("run rpm -qpl");
    let files = String::from_utf8_lossy(&files.stdout);
    assert!(files.lines().any(|line| line == "/usr/bin/demo-bin"));
    assert!(files.lines().any(|line| line == "/usr/bin/demo-alias"));
    assert!(files.lines().any(|line| line == "/etc/demo.conf"));
    assert!(files.lines().any(|line| line == "/var/lib/demo"));

    let package = rpm::Package::open(&path).expect("open RPM");
    let entries = package.metadata.get_file_entries().expect("file entries");
    let alias = entries
        .iter()
        .find(|entry| entry.path() == std::path::Path::new("/usr/bin/demo-alias"))
        .expect("symlink entry");
    assert_eq!(alias.linkto(), Some("demo-bin"));

    let metadata = Command::new("rpm")
        .args([
            "-qp",
            "--qf",
            "[%{FILENAMES} %{FILEUSERNAME} %{FILEGROUPNAME} %{FILEMODES:perms} %{FILECAPS}\n]",
        ])
        .arg(&path)
        .output()
        .expect("run rpm metadata query");
    let metadata = String::from_utf8_lossy(&metadata.stdout);
    assert!(
        metadata.contains("/usr/bin/demo-bin demo demo -rwxr-xr-x cap_net_raw=ep"),
        "metadata: {metadata}"
    );
    assert!(
        metadata.contains("/var/lib/demo demo demo drwxr-x---"),
        "metadata: {metadata}"
    );

    let requires = Command::new("rpm")
        .args(["-qp", "--requires"])
        .arg(&path)
        .output()
        .expect("run rpm --requires");
    let requires = String::from_utf8_lossy(&requires.stdout);
    assert!(requires.lines().any(|line| line.contains("glibc >= 2.39")));

    for (kind, expected) in [
        ("recommends", "demo-extra >= 1.0"),
        ("suggests", "demo-docs"),
        ("supplements", "demo-plugin"),
        ("enhances", "demo-host < 2.0"),
    ] {
        let output = Command::new("rpm")
            .args(["-qp", &format!("--{kind}")])
            .arg(&path)
            .output()
            .expect("run rpm weak dependency query");
        let output = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.lines().any(|line| line.contains(expected)),
            "{kind}: {output}"
        );
    }

    let obsoletes = Command::new("rpm")
        .args(["-qp", "--obsoletes"])
        .arg(&path)
        .output()
        .expect("run rpm --obsoletes");
    let obsoletes = String::from_utf8_lossy(&obsoletes.stdout);
    assert!(
        obsoletes.contains("old-demo < 1.0"),
        "obsoletes: {obsoletes}"
    );

    let conflicts = Command::new("rpm")
        .args(["-qp", "--conflicts"])
        .arg(&path)
        .output()
        .expect("run rpm --conflicts");
    let conflicts = String::from_utf8_lossy(&conflicts.stdout);
    assert!(conflicts.contains("demo-server"), "conflicts: {conflicts}");

    let scripts = Command::new("rpm")
        .args(["-qp", "--scripts"])
        .arg(&path)
        .output()
        .expect("run rpm --scripts");
    let scripts = String::from_utf8_lossy(&scripts.stdout);
    assert!(scripts.contains("echo installed"), "scripts: {scripts}");
}

#[test]
fn binary_package_is_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.rpm");
    let second = dir.path().join("second.rpm");
    write_binary_rpm(&spec(), &first).expect("first");
    write_binary_rpm(&spec(), &second).expect("second");
    assert_eq!(
        fs::read(&first).expect("read first"),
        fs::read(&second).expect("read second")
    );
}
