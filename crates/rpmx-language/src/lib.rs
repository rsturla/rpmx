pub mod matrix;
pub mod model;
pub mod plan;
pub mod steps;
pub mod template;

use rpmx_base::error::{codes, Error, Result};

pub use model::Recipe;
pub use plan::{
    normalize_dist_tag, plan, plan_with_options, Plan, PlanOptions, PlannedCell, PlannedOutput,
    PlannedStep, PlannedTest,
};

pub fn parse_recipe(text: &str) -> Result<Recipe> {
    rpmx_base::yaml::parse_strict(text, codes::YAML_001)
}

pub fn load_recipe(path: impl AsRef<std::path::Path>) -> Result<Recipe> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::IO_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    parse_recipe(&text)
}
