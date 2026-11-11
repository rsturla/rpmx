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
                data: b"schema: 1\n".to_vec(),
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

#[test]
fn recipe_metadata_survives_source_package_preparation_and_extraction() {
    use rpmx_core::source_package::{prepare_source_packages, RecipeInclude, SourcePackageOptions};

    let directory = tempfile::tempdir().expect("tempdir");
    let recipe_path = directory.path().join("package.yaml");
    let metadata = b"metadata:\n  eternal:\n    update:\n      asset: '${version}.tar.gz'\n      arbitrary: [true, null, 3]\n";
    fs::write(&recipe_path, "schema: 1\ninclude: [metadata.yaml]\npackage:\n  name: probe\n  version: '1.0'\n  release: 1\n  summary: probe\n  license: MIT\nbuild:\n  pipeline:\n    - runs: 'true'\noutputs:\n  - name: probe\n    metapackage: true\n").unwrap();
    fs::write(directory.path().join("metadata.yaml"), metadata).unwrap();
    fs::write(
        directory.path().join("rpmx-defaults.yaml"),
        "schema: 1\npackage: {}\nmetadata:\n  defaults-only: true\n",
    )
    .unwrap();
    let bundle = rpmx_core::load_recipe_bundle(&recipe_path).expect("recipe bundle");
    let plan = rpmx_core::plan(&bundle.recipe, "x86_64").expect("plan");
    let includes: Vec<_> = bundle
        .included_files()
        .map(|file| RecipeInclude {
            name: file.name.clone(),
            bytes: file.bytes.clone(),
        })
        .collect();
    let sources = tempfile::tempdir().unwrap();
    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &bundle.recipe,
        plan: &plan,
        recipe_bytes: &bundle.root_file().bytes,
        recipe_digest: Some(bundle.digest()),
        recipe_includes: &includes,
        step_root: None,
        sources: sources.path(),
        source_date_epoch: 1_700_000_000,
        root_lock: None,
        sysroot_lock: None,
        cross: None,
        sbom: false,
        replaced: None,
    })
    .expect("prepare source package");
    let srpm = directory.path().join("probe.src.rpm");
    write_srpm(&packages[0], &srpm).expect("write SRPM");
    let extracted = directory.path().join("extracted");
    extract_srpm(&srpm, &extracted).expect("extract SRPM");
    for file in &bundle.files {
        assert_eq!(fs::read(extracted.join(&file.name)).unwrap(), file.bytes);
    }
    let restored =
        rpmx_core::load_recipe_bundle(extracted.join("package.yaml")).expect("restored recipe");
    assert_eq!(restored.recipe.metadata, bundle.recipe.metadata);
    assert_eq!(restored.digest(), bundle.digest());
    assert!(restored.recipe.vars.is_empty());
}
