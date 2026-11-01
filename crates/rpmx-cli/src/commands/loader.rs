use std::path::{Path, PathBuf};

use rpmx_core::{load_recipe, plan_with_options, Error, Plan, PlanOptions, Recipe};

use crate::commands::PlanPolicy;

pub fn step_root(pipeline_dir: Option<&Path>, recipe: &Path) -> Option<PathBuf> {
    pipeline_dir
        .map(Path::to_path_buf)
        .or_else(|| recipe.parent().map(|parent| parent.join("pipelines")))
}

pub fn load_plan(
    file: &Path,
    arch: &str,
    pipeline_dir: Option<&Path>,
    allow_builtin_override: bool,
    policy: PlanPolicy<'_>,
) -> Result<(Recipe, Plan), Error> {
    let recipe = load_recipe(file)?;
    let root = step_root(pipeline_dir, file);
    let plan = plan_with_options(
        &recipe,
        PlanOptions {
            arch,
            step_root: root.as_deref(),
            allow_builtin_override,
            dist_tag: policy.dist_tag,
            build_release: policy.build_release,
        },
    )?;
    Ok((recipe, plan))
}
