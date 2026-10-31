use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{load_plan, PlanPolicy};
use crate::render;

pub fn run(
    file: &Path,
    pipeline_dir: Option<&Path>,
    policy: PlanPolicy<'_>,
    json: bool,
) -> Result<(), Error> {
    let (_, plan) = load_plan(file, "x86_64", pipeline_dir, false, policy)?;
    if json {
        let value = serde_json::json!({
            "ok": true,
            "cells": plan.cells.len(),
            "ids": plan.cells.iter().map(|cell| &cell.id).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_001, error.to_string()))?
        );
    } else {
        println!("{}", render::lint(&plan));
    }
    Ok(())
}
