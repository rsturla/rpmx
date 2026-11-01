pub mod binary_package;
pub mod build_env;
pub mod cas;
pub mod elf;
pub mod fetch;
pub mod package;
pub mod package_spec;
pub mod pkgmanifest;
pub mod profile;
pub mod root;
pub mod runner;
pub mod sandbox;
pub mod sbom;
pub mod source;
pub mod source_package;
pub mod srpm;
mod tar_guard;
pub mod terminal;
pub mod test_root;
pub mod test_runner;
pub mod tree;

pub use rpmx_base::{arch, error, guest, hash, yaml, Arch, Error, Result};

/// Hard ceiling for an untrusted archive staged or downloaded by the tool.
pub const MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub use rpmx_language::{
    load_recipe, matrix, model, normalize_dist_tag, parse_recipe, plan, plan_with_options, steps,
    template, Plan, PlanOptions, PlannedCell, PlannedOutput, PlannedStep, PlannedTest, Recipe,
};
