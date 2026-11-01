//! Inspect the built-in step library.

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{StepsListArgs, StepsShowArgs};

pub fn list(_args: StepsListArgs) -> Result<(), Error> {
    let names: Vec<_> = rpmx_core::steps::builtin_names().collect();
    if rpmx_core::output::json() {
        let document =
            crate::commands::report::success("steps", serde_json::json!({ "steps": names }));
        println!(
            "{}",
            serde_json::to_string_pretty(&document)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
        );
    } else {
        for name in names {
            println!("{name}");
        }
    }
    Ok(())
}

pub fn show(args: StepsShowArgs) -> Result<(), Error> {
    let source = rpmx_core::steps::builtin_source(&args.name).ok_or_else(|| {
        Error::new(
            codes::STEP_011,
            format!(
                "unknown built-in step '{}'; run `rpmx steps list`",
                args.name
            ),
        )
    })?;
    if rpmx_core::output::json() {
        let document = crate::commands::report::success(
            "steps",
            serde_json::json!({ "name": args.name, "source": source }),
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&document)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
        );
    } else {
        print!("{source}");
    }
    Ok(())
}
