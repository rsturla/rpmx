pub mod arch;
pub mod binary_package;
pub mod build_env;
pub mod cas;
pub mod elf;
pub mod error;
pub mod fetch;
pub mod guest;
pub mod hash;
pub mod matrix;
pub mod model;
pub mod package;
pub mod package_spec;
pub mod pkgmanifest;
pub mod plan;
pub mod profile;
pub mod root;
pub mod runner;
pub mod sandbox;
pub mod sbom;
pub mod source;
pub mod source_package;
pub mod srpm;
pub mod steps;
pub mod template;
pub mod test_root;
pub mod test_runner;
pub mod tree;
pub mod yaml;

use std::path::Path;

use crate::error::codes;
pub use arch::Arch;
pub use error::{Error, Result};
pub use model::Recipe;
pub use plan::{
    normalize_dist_tag, plan, plan_with_options, Plan, PlanOptions, PlannedCell, PlannedOutput,
    PlannedStep, PlannedTest,
};

pub fn parse_recipe(text: &str) -> Result<Recipe> {
    yaml::parse_strict(text, codes::YAML_001)
}

pub fn load_recipe(path: impl AsRef<Path>) -> Result<Recipe> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::IO_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    parse_recipe(&text)
}
