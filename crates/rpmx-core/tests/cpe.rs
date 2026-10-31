use std::fs;

use rpmx_core::{load_recipe, plan_with_options, PlanOptions};

fn recipe(cpe: &str) -> String {
    format!(
        "schema: 1\npackage:\n  name: demo\n  version: \"1.0\"\n  release: 1\n  summary: demo\n  license: MIT\n{cpe}\noutputs:\n  - name: demo\n    files:\n      include:\n        - /usr/bin/demo\n"
    )
}

fn plan(cpe: &str) -> Result<rpmx_core::Recipe, rpmx_core::Error> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("package.yaml");
    fs::write(&path, recipe(cpe)).expect("write");
    let recipe = load_recipe(&path)?;
    plan_with_options(
        &recipe,
        PlanOptions {
            arch: "x86_64",
            step_root: None,
            allow_builtin_override: false,
            dist_tag: None,
            build_release: None,
        },
    )?;
    Ok(recipe)
}

#[test]
fn cpe_is_rendered_with_version_and_release() {
    let recipe = plan("  cpe:\n    vendor: example\n    product: demo\n").expect("plan");
    let cpe = recipe.package.cpe.expect("cpe");
    assert_eq!(
        cpe.cpe23("1.0+git", "2"),
        "cpe:2.3:a:example:demo:1.0\\+git:2:*:*:*:*:*:*"
    );
}

#[test]
fn cpe_part_must_be_application_or_os() {
    let error = plan("  cpe:\n    part: h\n    vendor: example\n    product: demo\n")
        .expect_err("hardware part is not valid for a package");
    assert_eq!(error.code, "RPMX-SCHEMA-002");
}

#[test]
fn cpe_vendor_rejects_separators() {
    let error = plan("  cpe:\n    vendor: \"example:corp\"\n    product: demo\n")
        .expect_err("colon would change the CPE");
    assert_eq!(error.code, "RPMX-SCHEMA-002");
}
