#![allow(dead_code)]

use std::path::Path;

use rpmx_core::{load_recipe, plan_with_options, Error, Plan, PlanOptions, Recipe};

/// A stage tree with one binary and one documentation file.
pub fn stage() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("usr/bin")).expect("bin");
    std::fs::create_dir_all(dir.path().join("usr/share/doc/demo")).expect("doc");
    std::fs::write(dir.path().join("usr/bin/demo"), b"bin").expect("bin file");
    std::fs::write(dir.path().join("usr/share/doc/demo/README"), b"doc").expect("doc file");
    dir
}

/// Load a recipe from inline YAML and plan it for x86_64.
pub fn load(yaml: &str) -> (Recipe, Plan) {
    try_load(yaml).expect("recipe plans")
}

pub fn try_load(yaml: &str) -> Result<(Recipe, Plan), Error> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("package.yaml");
    std::fs::write(&path, yaml).expect("write recipe");
    let recipe = load_recipe(&path)?;
    let plan = plan_with_options(
        &recipe,
        PlanOptions {
            arch: "x86_64",
            step_root: None,
            allow_builtin_override: false,
            dist_tag: None,
            build_release: None,
        },
    )?;
    Ok((recipe, plan))
}

/// Plan inline YAML with a pipeline directory.
pub fn plan_with_root(yaml: &str, root: &Path) -> Result<Plan, Error> {
    let recipe = rpmx_core::parse_recipe(yaml)?;
    plan_with_options(
        &recipe,
        PlanOptions {
            arch: "x86_64",
            step_root: Some(root),
            allow_builtin_override: false,
            dist_tag: None,
            build_release: None,
        },
    )
}
