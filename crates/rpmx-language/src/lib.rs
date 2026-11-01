pub mod bundle;
pub mod matrix;
pub mod model;
pub mod plan;
pub mod steps;
pub mod template;

use std::path::Path;

use rpmx_base::error::{codes, Result};

pub use bundle::{load_recipe_bundle, BundleFile, RecipeBundle};
pub use model::Recipe;
pub use plan::{
    normalize_dist_tag, plan, plan_with_options, Plan, PlanOptions, PlannedCell, PlannedOutput,
    PlannedStep, PlannedTest,
};

pub fn parse_recipe(text: &str) -> Result<Recipe> {
    rpmx_base::yaml::parse_strict(text, codes::YAML_001)
}

/// Load a recipe, following `include:` entries.
pub fn load_recipe(path: impl AsRef<Path>) -> Result<Recipe> {
    load_recipe_bundle(path).map(|bundle| bundle.recipe)
}
