use std::io::Read;

use flate2::read::GzDecoder;
use rpmx_core::package_spec::{BinaryPackageFile, BinaryPackageSpec};
use rpmx_rpm::repo::{generate, RepoOptions};
use rpmx_rpm::write_binary_rpm;

fn spec() -> BinaryPackageSpec {
    BinaryPackageSpec {
        name: "demo".to_string(),
        version: "1.0".to_string(),
        release: "1".to_string(),
        epoch: None,
        arch: "x86_64".to_string(),
        summary: "demo".to_string(),
        license: "MIT".to_string(),
        description: "demo".to_string(),
        url: None,
        source_rpm: "demo-1.0-1.src.rpm".to_string(),
        source_date_epoch: 1_700_000_000,
        files: vec![BinaryPackageFile {
            path: "/usr/bin/demo".to_string(),
            data: b"#!/bin/sh\n".to_vec(),
            mode: 0o755,
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
            requires: vec!["/bin/sh".to_string()],
            recommends: vec!["demo-extra = 2:1.0-3".to_string()],
            suggests: vec!["demo-docs".to_string()],
            supplements: vec!["demo-plugin".to_string()],
            enhances: vec!["demo-host < 2.0".to_string()],
            provides: vec!["demo".to_string()],
            obsoletes: Vec::new(),
            conflicts: Vec::new(),
        },
        scriptlets: Default::default(),
    }
}

#[test]
fn repository_metadata_includes_weak_dependencies() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rpms = dir.path().join("rpms");
    let output = dir.path().join("repo");
    std::fs::create_dir(&rpms).expect("create rpm dir");
    write_binary_rpm(&spec(), &rpms.join("demo-1.0-1.x86_64.rpm")).expect("write RPM");

    generate(RepoOptions {
        rpms: &rpms,
        output: &output,
        signing_key: None,
        timestamp: 1_700_000_000,
    })
    .expect("generate repository");

    let compressed = std::fs::read(output.join("repodata/primary.xml.gz")).expect("read primary");
    let mut primary = String::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_string(&mut primary)
        .expect("decompress primary");

    for (element, expected) in [
        ("recommends", "demo-extra"),
        ("suggests", "demo-docs"),
        ("supplements", "demo-plugin"),
        ("enhances", "demo-host"),
    ] {
        assert!(
            primary.contains(&format!("<rpm:{element}>")),
            "{element} element missing from {primary}"
        );
        assert!(
            primary.contains(expected),
            "{element} entry '{expected}' missing from {primary}"
        );
    }
}

#[test]
fn repository_metadata_splits_dependency_evr() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rpms = dir.path().join("rpms");
    let output = dir.path().join("repo");
    std::fs::create_dir(&rpms).expect("create rpm dir");
    write_binary_rpm(&spec(), &rpms.join("demo-1.0-1.x86_64.rpm")).expect("write RPM");

    generate(RepoOptions {
        rpms: &rpms,
        output: &output,
        signing_key: None,
        timestamp: 1_700_000_000,
    })
    .expect("generate repository");

    let compressed = std::fs::read(output.join("repodata/primary.xml.gz")).expect("read primary");
    let mut primary = String::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_string(&mut primary)
        .expect("decompress primary");

    assert!(
        primary.contains("epoch=\"2\" ver=\"1.0\" rel=\"3\""),
        "dependency EVR was not split: {primary}"
    );
    assert!(
        !primary.contains("ver=\"2:1.0-3\""),
        "dependency EVR kept its epoch/release in ver: {primary}"
    );
}
