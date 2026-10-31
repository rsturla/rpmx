use std::fs;

use rpmx_core::model::Cpe;
use rpmx_core::sbom::{
    native_spdx, BinaryRef, BuildSbom, Generator, PackageSbom, SourceRef, SBOM_DIR,
};

#[test]
fn package_spdx_describes_the_package_and_its_sources() {
    let sources = [SourceRef {
        path: "demo-1.0.tar.gz",
        digest: "sha512:abcd",
    }];
    let cpe = Cpe {
        part: "a".to_string(),
        vendor: "example".to_string(),
        product: "demo".to_string(),
        edition: None,
        language: None,
        sw_edition: None,
        target_sw: None,
        target_hw: None,
        other: None,
    };
    let text = native_spdx()
        .package(&PackageSbom {
            name: "demo",
            version: "1.0",
            release: "1",
            arch: "x86_64",
            license: "MIT",
            source_rpm: "demo-1.0-1.src.rpm",
            recipe_digest: "sha512:recipe",
            source_date_epoch: 0,
            sources: &sources,
            cpe: Some(&cpe),
        })
        .expect("package sbom");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["spdxVersion"], "SPDX-2.3");
    assert_eq!(value["creationInfo"]["created"], "1970-01-01T00:00:00Z");
    assert_eq!(value["packages"][0]["SPDXID"], "SPDXRef-Package-demo");
    assert_eq!(
        value["packages"][0]["externalRefs"][0]["referenceLocator"],
        "pkg:rpm/demo@1.0-1?arch=x86_64"
    );
    assert_eq!(
        value["packages"][0]["externalRefs"][1]["referenceType"],
        "cpe23Type"
    );
    assert_eq!(
        value["packages"][0]["externalRefs"][1]["referenceLocator"],
        "cpe:2.3:a:example:demo:1.0:1:*:*:*:*:*:*"
    );
    let relationships = value["relationships"].as_array().expect("relationships");
    assert!(relationships.iter().any(|relationship| {
        relationship["relationshipType"] == "GENERATED_FROM"
            && relationship["relatedSpdxElement"] == "SPDXRef-Source-0"
    }));
    assert!(SBOM_DIR.starts_with("/usr/share"));
}

#[test]
fn build_spdx_links_binaries_to_the_source_and_root() {
    let work = tempfile::tempdir().expect("work");
    let rpm = work.path().join("demo-1.0-1.x86_64.rpm");
    fs::write(&rpm, b"rpm payload").expect("rpm");
    let binaries = [BinaryRef {
        name: "demo",
        version: "1.0",
        release: "1",
        arch: "x86_64",
        license: "MIT",
        path: &rpm,
    }];
    let sources = [];
    let text = native_spdx()
        .build(&BuildSbom {
            name: "demo",
            version: "1.0",
            release: "1",
            arch: "x86_64",
            license: "MIT",
            recipe_digest: "sha512:recipe",
            source_date_epoch: 1_700_000_000,
            sources: &sources,
            binaries: &binaries,
            root_lock: None,
            signing_key: Some("ABCDEF0123456789"),
            cpe: None,
        })
        .expect("build sbom");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["spdxVersion"], "SPDX-2.3");
    assert_eq!(
        value["relationships"][0]["relatedSpdxElement"],
        "SPDXRef-SourcePackage-demo"
    );
    let relationships = value["relationships"].as_array().expect("relationships");
    assert!(relationships.iter().any(|relationship| {
        relationship["spdxElementId"] == "SPDXRef-Binary-demo-x86-64"
            && relationship["relationshipType"] == "GENERATED_FROM"
    }));
    let bin = value["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|package| package["SPDXID"] == "SPDXRef-Binary-demo-x86-64")
        .expect("binary package");
    assert_eq!(
        bin["annotations"][0]["comment"],
        "OpenPGP signature by key ABCDEF0123456789"
    );
}
