use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{load_plan, PlanPolicy};
use crate::render;

pub fn run(
    file: &Path,
    arch: &str,
    pipeline_dir: Option<&Path>,
    policy: PlanPolicy<'_>,
    json: bool,
) -> Result<(), Error> {
    let (_, plan) = load_plan(file, arch, pipeline_dir, false, policy)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&plan)
                .map_err(|error| Error::new(codes::CLI_002, error.to_string()))?
        );
    } else {
        render::plan(&plan);
    }
    Ok(())
}
