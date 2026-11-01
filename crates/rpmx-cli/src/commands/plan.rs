use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{load_plan, resolve_policy, PlanArgs, PolicyInput};
use crate::render;

pub fn run(args: PlanArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let (_, plan) = load_plan(
        &args.file,
        &args.arch,
        pipeline_dir,
        false,
        resolved.plan_policy(),
    )?;
    if rpmx_core::output::json() {
        let value = crate::commands::report::success(
            "plan",
            serde_json::json!({ "plan": serde_json::to_value(&plan).map_err(|error| {
                Error::new(codes::CLI_002, error.to_string())
            })? }),
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_002, error.to_string()))?
        );
    } else {
        render::plan(&plan);
    }
    Ok(())
}
