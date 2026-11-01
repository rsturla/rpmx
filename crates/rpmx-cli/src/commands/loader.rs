use std::path::{Path, PathBuf};

use rpmx_core::{load_recipe_bundle, plan_with_options, Error, Plan, PlanOptions, RecipeBundle};

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
) -> Result<(RecipeBundle, Plan), Error> {
    let bundle = load_recipe_bundle(file)?;
    let root = step_root(pipeline_dir, &bundle.root);
    // A named root profile carries the distribution tag even when the caller
    // did not pass --profile. Leaking one short string per plan keeps
    // `PlanPolicy` borrowed while making every command agree on the release.
    let dist_tag = match policy.dist_tag {
        Some(tag) => Some(tag),
        None => crate::commands::autoroot::profile_dist_tag(&bundle.recipe, &bundle.root)?
            .map(|tag| &*Box::leak(tag.into_boxed_str())),
    };
    let plan = plan_with_options(
        &bundle.recipe,
        PlanOptions {
            arch,
            step_root: root.as_deref(),
            allow_builtin_override,
            dist_tag,
            build_release: policy.build_release,
        },
    )?;
    Ok((bundle, plan))
}
