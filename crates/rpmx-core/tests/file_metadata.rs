use std::fs;

mod common;

use common::load;
use rpmx_core::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use rpmx_core::{load_recipe, plan_with_options, PlanOptions};

fn recipe(files: &str) -> String {
    format!(
        "schema: 1\npackage:\n  name: demo\n  version: \"1.0\"\n  release: 1\n  summary: demo\n  license: MIT\noutputs:\n  - name: demo\n    files:\n      include:\n        - /usr/bin/demo\n{files}"
    )
}

#[test]
fn attributes_and_directories_reach_the_prepared_package() {
    let (recipe, plan) = load(&recipe(
        "      directories:\n        - path: /var/lib/demo\n          mode: \"0750\"\n          user: demo\n          group: demo\n      attributes:\n        - path: /usr/bin/demo\n          mode: \"0750\"\n          user: demo\n          group: demo\n          caps: cap_net_raw=ep\n",
    ));
    let stage = tempfile::tempdir().expect("stage");
    fs::create_dir_all(stage.path().join("usr/bin")).expect("bin");
    fs::write(stage.path().join("usr/bin/demo"), b"#!/bin/sh\n").expect("file");

    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: stage.path(),
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
    })
    .expect("prepare");

    assert_eq!(packages[0].files[0].mode, 0o750);
    assert_eq!(packages[0].files[0].user.as_deref(), Some("demo"));
    assert_eq!(packages[0].files[0].group.as_deref(), Some("demo"));
    assert_eq!(packages[0].files[0].caps.as_deref(), Some("cap_net_raw=ep"));
    assert_eq!(packages[0].directories[0].path, "/var/lib/demo");
    assert_eq!(packages[0].directories[0].mode, 0o750);
}

#[test]
fn attributes_for_a_missing_file_are_rejected() {
    let (recipe, plan) = load(&recipe(
        "      attributes:\n        - path: /usr/bin/other\n          mode: \"0755\"\n",
    ));
    let stage = tempfile::tempdir().expect("stage");
    fs::create_dir_all(stage.path().join("usr/bin")).expect("bin");
    fs::write(stage.path().join("usr/bin/demo"), b"#!/bin/sh\n").expect("file");

    let error = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: stage.path(),
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
    })
    .expect_err("missing attribute target");

    assert_eq!(error.code, "RPMX-OUTPUT-018");
    assert!(error.message.contains("/usr/bin/other"), "{error}");
}

#[test]
fn invalid_modes_are_rejected_while_planning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("package.yaml");
    fs::write(
        &path,
        recipe("      directories:\n        - path: /var/lib/demo\n          mode: \"0999\"\n"),
    )
    .expect("write");
    let recipe = load_recipe(&path).expect("load");
    let error = plan_with_options(
        &recipe,
        PlanOptions {
            arch: "x86_64",
            step_root: None,
            allow_builtin_override: false,
            dist_tag: None,
            build_release: None,
        },
    )
    .expect_err("invalid mode");
    assert_eq!(error.code, "RPMX-OUTPUT-016");
}
