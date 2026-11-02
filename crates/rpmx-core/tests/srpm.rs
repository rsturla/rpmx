use std::fs;

use rpmx_core::srpm::{
    load_lock_file, lock_json, manifest_json, snapshot_dir, LockBuild, LockBuildRef, LockStep,
};

#[test]
fn snapshot_is_flat_sorted_and_digested() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("b.tar.gz"), b"b").expect("b");
    fs::write(dir.path().join("a.tar.gz"), b"a").expect("a");

    let files = snapshot_dir(dir.path()).expect("snapshot");
    let paths: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["a.tar.gz", "b.tar.gz"]);
    assert!(files[0].digest.starts_with("sha512:"));
    assert_eq!(files[1].data, b"b");
}

#[test]
fn nested_source_paths_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join("sub")).expect("dir");
    fs::write(dir.path().join("sub/extra.txt"), b"x").expect("file");

    let error = snapshot_dir(dir.path()).expect_err("nested source is rejected");
    assert_eq!(error.code, "RPMX-SOURCE-020");
}

#[test]
fn lock_and_manifest_are_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("a.tar.gz"), b"a").expect("a");
    let files = snapshot_dir(dir.path()).expect("snapshot");
    let steps = [LockStep {
        name: "acme/strip".to_string(),
        digest: "sha512:abc".to_string(),
    }];

    let build = LockBuild::default();
    let first = lock_json(
        "sha512:recipe",
        "1.0",
        "1.fc44+build7",
        Some(".fc44"),
        Some(7),
        1_700_000_000,
        &files,
        &steps,
        LockBuildRef {
            root_lock: None,
            cross: None,
            build: &build,
        },
    )
    .expect("lock");
    let second = lock_json(
        "sha512:recipe",
        "1.0",
        "1.fc44+build7",
        Some(".fc44"),
        Some(7),
        1_700_000_000,
        &files,
        &steps,
        LockBuildRef {
            root_lock: None,
            cross: None,
            build: &build,
        },
    )
    .expect("lock");
    assert_eq!(first, second);
    assert!(first.contains("\"release\": \"1.fc44+build7\""));
    assert!(first.contains("\"dist_tag\": \".fc44\""));
    assert!(first.contains("\"build_release\": 7"));

    let manifest = manifest_json(
        "sha512:recipe",
        "sha512:lock",
        "sha512:bundled",
        1_700_000_000,
        &files,
        &steps,
        None,
    )
    .expect("manifest");
    assert!(manifest.contains("\"name\": \"acme/strip\""));
    assert!(manifest.contains("\"path\": \"package.lock.json\""));

    let path = dir.path().join("package.lock.json");
    fs::write(&path, &first).expect("write lock");
    let loaded = load_lock_file(&path).expect("load lock");
    assert_eq!(loaded.release, "1.fc44+build7");
    assert_eq!(loaded.dist_tag.as_deref(), Some(".fc44"));
    assert_eq!(loaded.build_release, Some(7));
}
