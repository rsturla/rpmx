use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::report;
use crate::commands::BootstrapPlanArgs;
use crate::render;

pub fn run(args: BootstrapPlanArgs) -> Result<(), Error> {
    let plan = rpmx_core::bootstrap::plan_manifest(&args.manifest, args.arch.as_deref())?;
    if rpmx_core::output::json() {
        let value = report::success(
            "bootstrap-plan",
            serde_json::to_value(&plan)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?,
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
        );
    } else {
        render::bootstrap_plan(&plan);
    }
    Ok(())
}
