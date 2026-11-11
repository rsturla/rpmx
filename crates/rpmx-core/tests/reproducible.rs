use std::fs;

use rpmx_core::load_recipe;

fn recipe(extra: &str) -> String {
    format!(
        "schema: 2\npackage:\n  name: demo\n  version: \"1.0\"\n  release: 1\n  summary: demo\n  license: MIT\n{extra}"
    )
}

#[test]
fn source_date_epoch_is_read_from_the_recipe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("package.yaml");
    fs::write(&path, recipe("  source-date-epoch: 1704067200\n")).expect("write");

    let recipe = load_recipe(&path).expect("load");
    assert_eq!(recipe.package.source_date_epoch, Some(1_704_067_200));
}

#[test]
fn source_date_epoch_is_optional() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("package.yaml");
    fs::write(&path, recipe("")).expect("write");

    let recipe = load_recipe(&path).expect("load");
    assert_eq!(recipe.package.source_date_epoch, None);
}

#[test]
fn source_date_epoch_rejects_non_numbers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("package.yaml");
    fs::write(&path, recipe("  source-date-epoch: soon\n")).expect("write");

    let error = load_recipe(&path).expect_err("string is not a timestamp");
    assert_eq!(error.code, "RPMX-YAML-001");
}
