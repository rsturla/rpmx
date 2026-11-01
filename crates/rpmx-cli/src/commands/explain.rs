//! Show why the plan looks the way it does.

use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{autoroot, load_plan, resolve_policy, ExplainArgs, PolicyInput};

pub fn run(args: ExplainArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    let (recipe, plan) = load_plan(&args.file, &args.arch, pipeline_dir, false, policy)?;

    let value = match args.topic.as_str() {
        "policy" => serde_json::json!({
            "dist_tag": plan.dist_tag,
            "build_release": plan.build_release,
            "profile": args
                .policy
                .profile
                .as_ref()
                .map(|path| path.display().to_string())
                .or_else(|| recipe.build.root.as_ref().and_then(|root| root.profile.clone())),
            "release": plan.cells.iter().map(|cell| &cell.release).collect::<Vec<_>>(),
            "source_date_epoch": crate::commands::source_date_epoch(&recipe, None),
        }),
        "root" => match autoroot::requirement(
            &recipe,
            &plan,
            &args.file,
            args.policy.profile.as_deref(),
            &args.arch,
        )? {
            Some(requirement) => serde_json::json!({
                "profile": requirement.name,
                "requirement": requirement.digest,
                "releasever": requirement.profile.releasever,
                "packages": requirement.profile.packages.install,
            }),
            None => serde_json::json!({ "root": null }),
        },
        "steps" => serde_json::json!({
            "source_assembly": plan.source_assembly.as_ref().map(|source| {
                source.steps.iter().map(step).collect::<Vec<_>>()
            }),
            "cells": plan.cells.iter().map(|cell| serde_json::json!({
                "id": cell.id,
                "steps": cell.pipeline.iter().map(step).collect::<Vec<_>>(),
                "tests": cell.tests.iter().map(|test| serde_json::json!({
                    "name": test.name,
                    "stage": test.stage,
                    "steps": test.pipeline.iter().map(step).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }),
        "matrix" => serde_json::json!({
            "cells": plan.cells.iter().map(|cell| serde_json::json!({
                "id": cell.id,
                "slug": cell.slug,
                "package": cell.package,
                "version": cell.version,
                "release": cell.release,
            })).collect::<Vec<_>>(),
        }),
        "requires" => serde_json::json!({
            "outputs": plan.cells.iter().flat_map(|cell| cell.outputs.iter().map(|output| serde_json::json!({
                "cell": cell.id,
                "name": output.name,
                "run": &output.dependencies.requires,
                "recommends": &output.dependencies.recommends,
                "provides": &output.dependencies.provides,
                "conflicts": &output.dependencies.conflicts,
                "obsoletes": &output.dependencies.obsoletes,
            }))).collect::<Vec<_>>(),
        }),
        "files" => serde_json::json!({
            "outputs": plan.cells.iter().flat_map(|cell| cell.outputs.iter().map(|output| serde_json::json!({
                "cell": cell.id,
                "name": output.name,
                "include": output.files,
                "exclude": output.exclude,
            }))).collect::<Vec<_>>(),
        }),
        other => {
            return Err(Error::new(
                codes::CLI_006,
                format!("unknown explain topic '{other}'"),
            ))
        }
    };

    if args.json {
        let document = crate::commands::report::success("explain", value);
        println!(
            "{}",
            serde_json::to_string_pretty(&document)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
        );
    } else {
        println!(
            "{}",
            serde_yaml_ng::to_string(&value)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
        );
    }
    Ok(())
}

fn step(step: &rpmx_core::PlannedStep) -> serde_json::Value {
    serde_json::json!({
        "uses": step.uses,
        "command": step.command,
        "args": step.args,
        "needs": step.needs,
        "digest": step.step_digest,
        "builtin": step.builtin,
    })
}
