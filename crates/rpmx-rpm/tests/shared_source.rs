use rpmx_core::package_spec::{SourcePackageFile, SourcePackageSpec};
use rpmx_core::source_package::merge_target_packages;
use rpmx_core::srpm::{load_target_lock, sha512_hex};

fn variant(arch: &str) -> SourcePackageSpec {
    let root = format!("{{\"arch\":\"{arch}\"}}\n");
    let lock = serde_json::json!({
        "schema": 1, "recipe_digest": "recipe", "version": "1", "release": "1",
        "source_date_epoch": 1704067200, "sources": [], "steps": [],
        "root_lock": {"path": "root.lock.json", "digest": sha512_hex(root.as_bytes())},
        "build": {"packages": [], "sysroot": [], "tests": []}
    });
    let contents = [
        ("package.yaml", "schema: 2\n".to_string()),
        ("RPMX-SOURCE.tar.gz", "shared source snapshot".to_string()),
        ("root.lock.json", root),
        ("package.lock.json", lock.to_string()),
        (
            "RPMX-MANIFEST.json",
            serde_json::json!({
                "schema": 1, "source_date_epoch": 1704067200,
                "lock": {"path": "package.lock.json", "digest": "old"},
                "root_lock": {"path": "root.lock.json", "digest": "old"}
            })
            .to_string(),
        ),
    ];
    SourcePackageSpec {
        name: "shared-probe".into(),
        version: "1".into(),
        release: "1".into(),
        epoch: None,
        summary: "Shared source probe".into(),
        description: "A shared source package.".into(),
        license: "MIT".into(),
        arch: arch.into(),
        source_date_epoch: 1704067200,
        files: contents
            .into_iter()
            .map(|(path, data)| SourcePackageFile {
                path: path.into(),
                data: data.into_bytes(),
                mode: 0o644,
            })
            .collect(),
    }
}

#[test]
fn shared_srpm_is_independent_of_target_order_and_selects_verified_locks() {
    let a = merge_target_packages(vec![variant("x86_64"), variant("aarch64")]).unwrap();
    let b = merge_target_packages(vec![variant("aarch64"), variant("x86_64")]).unwrap();
    let work = tempfile::tempdir().unwrap();
    let first = work.path().join("first.src.rpm");
    let second = work.path().join("second.src.rpm");
    rpmx_rpm::write_srpm(&a, &first).unwrap();
    rpmx_rpm::write_srpm(&b, &second).unwrap();
    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap()
    );
    assert_eq!(rpmx_rpm::package_arch(&first).unwrap(), "noarch");
    let extracted = work.path().join("extracted");
    std::fs::create_dir(&extracted).unwrap();
    rpmx_rpm::extract::extract_srpm(&first, &extracted).unwrap();
    for arch in ["x86_64", "aarch64"] {
        let lock = load_target_lock(&extracted, arch).unwrap();
        assert_eq!(
            lock.root_lock.unwrap().path,
            format!("root-{arch}.lock.json")
        );
    }
    assert!(load_target_lock(&extracted, "riscv64").is_err());
    std::fs::write(extracted.join("package-aarch64.lock.json"), "{}").unwrap();
    assert!(load_target_lock(&extracted, "aarch64").is_err());
    assert!(load_target_lock(&extracted, "x86_64").is_ok());
}

#[test]
fn shared_srpm_rejects_different_source_snapshots() {
    let mut different = variant("aarch64");
    different
        .files
        .iter_mut()
        .find(|file| file.path == "RPMX-SOURCE.tar.gz")
        .unwrap()
        .data
        .push(1);
    assert!(merge_target_packages(vec![variant("x86_64"), different]).is_err());
}

#[test]
fn target_selection_retains_legacy_package_locks() {
    let work = tempfile::tempdir().unwrap();
    let package = variant("x86_64");
    for file in package.files {
        std::fs::write(work.path().join(file.path), file.data).unwrap();
    }
    assert_eq!(
        load_target_lock(work.path(), "x86_64").unwrap().version,
        "1"
    );
}

#[test]
fn shared_sources_do_not_need_embedded_build_environments() {
    let mut variants = vec![variant("x86_64"), variant("aarch64")];
    for variant in &mut variants {
        variant.files.retain(|file| file.path != "root.lock.json");
        let file = variant
            .files
            .iter_mut()
            .find(|file| file.path == "package.lock.json")
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&file.data).unwrap();
        value.as_object_mut().unwrap().remove("root_lock");
        file.data = serde_json::to_vec(&value).unwrap();
    }
    let package = merge_target_packages(variants).unwrap();
    assert!(package
        .files
        .iter()
        .all(|file| !file.path.starts_with("root-")));
    let directory = tempfile::tempdir().unwrap();
    for file in &package.files {
        std::fs::write(directory.path().join(&file.path), &file.data).unwrap();
    }
    for arch in ["x86_64", "aarch64"] {
        assert!(load_target_lock(directory.path(), arch)
            .unwrap()
            .root_lock
            .is_none());
    }
}
