use std::fs;

use rpmx_core::package::{scan_stage, split, OutputSpec};

fn stage() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join("usr/bin")).expect("bin");
    fs::create_dir_all(dir.path().join("usr/share/doc/demo")).expect("doc");
    fs::write(dir.path().join("usr/bin/demo"), b"bin").expect("bin file");
    fs::write(dir.path().join("usr/share/doc/demo/README"), b"doc").expect("doc file");
    dir
}

#[test]
fn stage_files_are_assigned_to_one_output() {
    let dir = stage();
    let files = scan_stage(dir.path()).expect("scan");
    let outputs = [
        OutputSpec {
            name: "demo".to_string(),
            include: vec!["/usr/bin/**".to_string()],
            exclude: vec![],
            metapackage: false,
        },
        OutputSpec {
            name: "demo-doc".to_string(),
            include: vec!["/usr/share/doc/**".to_string()],
            exclude: vec![],
            metapackage: false,
        },
    ];
    let split = split(&files, &outputs).expect("split");
    assert_eq!(split[0].files[0].path, "/usr/bin/demo");
    assert_eq!(split[1].files[0].path, "/usr/share/doc/demo/README");
}

#[test]
fn unclaimed_stage_file_is_an_error() {
    let dir = stage();
    let files = scan_stage(dir.path()).expect("scan");
    let outputs = [OutputSpec {
        name: "demo".to_string(),
        include: vec!["/usr/bin/**".to_string()],
        exclude: vec![],
        metapackage: false,
    }];
    let error = split(&files, &outputs).expect_err("doc file is unclaimed");
    assert_eq!(error.code, "RPMX-OUTPUT-010");
}

#[test]
fn overlapping_outputs_are_an_error() {
    let dir = stage();
    let files = scan_stage(dir.path()).expect("scan");
    let outputs = [
        OutputSpec {
            name: "a".to_string(),
            include: vec!["/usr/bin/**".to_string()],
            exclude: vec![],
            metapackage: false,
        },
        OutputSpec {
            name: "b".to_string(),
            include: vec!["/usr/bin/demo".to_string()],
            exclude: vec![],
            metapackage: false,
        },
    ];
    let error = split(&files, &outputs).expect_err("duplicate claim");
    assert_eq!(error.code, "RPMX-OUTPUT-011");
}

#[test]
fn empty_metapackage_is_allowed() {
    let dir = stage();
    let files = scan_stage(dir.path()).expect("scan");
    let outputs = [
        OutputSpec {
            name: "demo".to_string(),
            include: vec!["/usr/**".to_string()],
            exclude: vec![],
            metapackage: false,
        },
        OutputSpec {
            name: "demo-meta".to_string(),
            include: vec![],
            exclude: vec![],
            metapackage: true,
        },
    ];
    let split = split(&files, &outputs).expect("metapackage split");
    assert!(split[1].files.is_empty());
}

#[test]
fn symlinks_are_scanned_and_split() {
    let dir = stage();
    std::os::unix::fs::symlink("demo", dir.path().join("usr/bin/demo-alias")).expect("symlink");
    let files = scan_stage(dir.path()).expect("scan");
    let alias = files
        .iter()
        .find(|file| file.path == "/usr/bin/demo-alias")
        .expect("symlink entry");
    assert_eq!(alias.target.as_deref(), Some("demo"));
    assert!(alias.data.is_empty());

    let outputs = [
        OutputSpec {
            name: "demo".to_string(),
            include: vec!["/usr/bin/**".to_string()],
            exclude: vec![],
            metapackage: false,
        },
        OutputSpec {
            name: "demo-doc".to_string(),
            include: vec!["/usr/share/doc/**".to_string()],
            exclude: vec![],
            metapackage: false,
        },
    ];
    let split = split(&files, &outputs).expect("split");
    assert!(split[0]
        .files
        .iter()
        .any(|file| file.path == "/usr/bin/demo-alias"));
}
