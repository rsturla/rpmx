use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{load_plan, resolve_policy, LintArgs, PolicyInput};
use crate::render;

pub fn run(args: LintArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
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
    if args.json {
        let value = serde_json::json!({
            "schema": 1,
            "ok": true,
            "command": "lint",
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
