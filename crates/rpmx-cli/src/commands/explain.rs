//! Show why the plan looks the way it does.

use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{load_plan, resolve_policy, ExplainArgs, PolicyInput};
use rpmx_root::recipe as autoroot;

pub fn run(args: ExplainArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    let arch = crate::commands::command_arch(
        &args.file,
        args.arch.as_deref(),
        args.policy.profile.as_deref(),
    )?;
    let (bundle, plan) = load_plan(&args.file, &arch, pipeline_dir, false, true, policy)?;

    let value = match args.topic.as_str() {
        "policy" => serde_json::json!({
            "dist_tag": plan.dist_tag,
            "build_release": plan.build_release,
            "profile": args
                .policy
                .profile
                .as_ref()
                .map(|path| path.display().to_string())
                .or_else(|| bundle.recipe.build.root.as_ref().and_then(|root| root.profile.clone())),
            "release": plan.cells.iter().map(|cell| &cell.release).collect::<Vec<_>>(),
            "source_date_epoch": crate::commands::source_date_epoch(&bundle.recipe, None),
        }),
        "root" => match autoroot::requirement(
            &bundle.recipe,
            &plan,
            &bundle.root,
            args.policy.profile.as_deref(),
            &arch,
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
                "build_tests": cell.build_tests,
                "tests": cell.tests.iter().map(|test| serde_json::json!({
                    "name": test.name,
                    "output": test.output,
                    "mode": test.mode,
                    "network": test.network,
                    "packages": test.packages,
                    "environment": test.environment,
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
        "files" => {
            let main = plan
                .cells
                .first()
                .and_then(|cell| cell.outputs.first())
                .map(|output| output.name.clone());
            serde_json::json!({
                "main": main,
                "outputs": plan.cells.iter().flat_map(|cell| cell.outputs.iter().map(|output| serde_json::json!({
                    "cell": cell.id,
                    "name": output.name,
                    "main": Some(&output.name) == main.as_ref(),
                    "include": output.files,
                    "exclude": output.exclude,
                }))).collect::<Vec<_>>(),
            })
        }
        "components" => {
            let inventory = match &args.source {
                Some(source) => {
                    if !source.is_dir() {
                        return Err(Error::new(
                            codes::CLI_007,
                            format!("source workspace {} is not a directory", source.display()),
                        ));
                    }
                    let cells = plan.cells.iter().collect::<Vec<_>>();
                    rpmx_core::components::discover(source, &cells)?
                }
                None => rpmx_core::components::Inventory::from_plan(&plan),
            };
            let outputs = plan
                .cells
                .iter()
                .flat_map(|cell| {
                    cell.outputs.iter().map(|output| {
                        let provides = if output.generators.bundled {
                            inventory
                                .provides_for(output)
                                .into_iter()
                                .filter(|provide| !output.generators.exclude.contains(provide))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let components = inventory
                            .components_for(output)
                            .map(|component| {
                                serde_json::json!({
                                    "ecosystem": component.ecosystem,
                                    "name": component.name,
                                    "version": component.version,
                                    "purl": component.purl,
                                    "license": component.license,
                                    "scope": component.scope,
                                    "confidence": component.confidence,
                                    "source": component.source,
                                    "evidence": component.evidence,
                                    "provide": component.provide(),
                                })
                            })
                            .collect::<Vec<_>>();
                        serde_json::json!({
                            "cell": cell.id,
                            "name": output.name,
                            "bundled": output.generators.bundled,
                            "provides": provides,
                            "components": components,
                        })
                    })
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "source": args.source.as_ref().map(|path| path.display().to_string()),
                "outputs": outputs,
            })
        }
        other => {
            return Err(Error::new(
                codes::CLI_006,
                format!("unknown explain topic '{other}'"),
            ))
        }
    };

    if rpmx_core::output::json() {
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
        "label": step.label,
        "uses": step.uses,
        "command": step.command,
        "args": step.args,
        "needs": step.needs,
        "digest": step.step_digest,
        "builtin": step.builtin,
    })
}
