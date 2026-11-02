use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use rpmx_core::error::codes;
use rpmx_core::model::Recipe;
use rpmx_core::plan::Plan;
use rpmx_core::Error;
use rpmx_core::RecipeBundle;

use crate::commands::{load_plan, resolve_policy, LintArgs, PolicyInput};
use crate::render;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize)]
struct Diagnostic {
    code: &'static str,
    severity: Severity,
    rule: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

impl Diagnostic {
    fn error(code: rpmx_core::error::Code, rule: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: code.as_str(),
            severity: Severity::Error,
            rule,
            message: message.into(),
            path: None,
        }
    }

    fn warning(
        code: rpmx_core::error::Code,
        rule: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.as_str(),
            severity: Severity::Warning,
            rule,
            message: message.into(),
            path: None,
        }
    }

    fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

pub fn run(args: LintArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let arch = crate::commands::command_arch(
        &args.file,
        args.arch.as_deref(),
        args.policy.profile.as_deref(),
    )?;
    let (bundle, plan) = load_plan(
        &args.file,
        &arch,
        pipeline_dir,
        false,
        true,
        resolved.plan_policy(),
    )?;
    let mut diagnostics = check(&bundle.recipe, &plan, &bundle.root);
    attach_owners(&mut diagnostics, &bundle);
    let errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .count();
    let warnings = diagnostics.len() - errors;
    let failed = errors > 0 || (args.strict && warnings > 0);

    if rpmx_core::output::json() {
        let value = serde_json::json!({
            "schema": 1,
            "ok": !failed,
            "command": "lint",
            "cells": plan.cells.len(),
            "ids": plan.cells.iter().map(|cell| &cell.id).collect::<Vec<_>>(),
            "summary": { "errors": errors, "warnings": warnings },
            "diagnostics": diagnostics,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_001, error.to_string()))?
        );
    } else if diagnostics.is_empty() {
        println!("{}", render::lint(&plan));
    } else {
        for diagnostic in &diagnostics {
            println!("{}", render_diagnostic(diagnostic));
        }
        println!("{} error(s), {} warning(s)", errors, warnings);
    }

    if failed {
        return Err(Error::new(
            codes::LINT_001,
            if errors > 0 {
                format!("{errors} lint error(s)")
            } else {
                format!("{warnings} lint warning(s) treated as errors by --strict")
            },
        )
        .reported());
    }
    Ok(())
}

/// Prefix a field path with the file that owns its top-level section.
fn attach_owners(diagnostics: &mut [Diagnostic], bundle: &RecipeBundle) {
    let root_dir = bundle.root.parent().unwrap_or(std::path::Path::new("."));
    for diagnostic in diagnostics {
        let Some(field) = &diagnostic.path else {
            continue;
        };
        let section = field.split(['.', '[']).next().unwrap_or(field);
        let key = if section == "source_assembly" {
            "source-assembly"
        } else {
            section
        };
        let Some(owner) = bundle.owner(key) else {
            continue;
        };
        if owner == bundle.root {
            continue;
        }
        let name = owner
            .strip_prefix(root_dir)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| owner.display().to_string());
        diagnostic.path = Some(format!("{name}: {field}"));
    }
}

fn render_diagnostic(diagnostic: &Diagnostic) -> String {
    let severity = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let mut line = format!(
        "{severity}[{}]: {}: {}",
        diagnostic.code, diagnostic.rule, diagnostic.message
    );
    if let Some(path) = &diagnostic.path {
        line.push_str(" (");
        line.push_str(path);
        line.push(')');
    }
    line
}

/// Semantic checks that the planner does not perform.
fn check(recipe: &Recipe, plan: &Plan, recipe_path: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let recipe_dir = recipe_path.parent().unwrap_or_else(|| Path::new("."));

    if let Some(context) = &recipe.context {
        match rpmx_core::source::context_root(recipe_dir, context) {
            Ok(root) => {
                if let Err(error) = rpmx_core::tree::context_digest(&root) {
                    diagnostics.push(
                        Diagnostic::error(error.code, "context/digest", error.message)
                            .at("context"),
                    );
                }
            }
            Err(error) => {
                diagnostics.push(
                    Diagnostic::error(error.code, "context/path", error.message).at("context"),
                );
            }
        }
    }

    if let Err(error) = spdx_syntax(&recipe.package.license) {
        diagnostics.push(
            Diagnostic::error(
                codes::LINT_010,
                "license/spdx",
                format!("package.license is not a valid SPDX expression: {error}"),
            )
            .at("package.license"),
        );
    }

    let summary = recipe.package.summary.trim();
    if summary.chars().count() < 2 {
        diagnostics.push(
            Diagnostic::error(
                codes::LINT_012,
                "summary/empty",
                "package.summary must say what the package is",
            )
            .at("package.summary"),
        );
    } else if summary.chars().count() > 80 {
        diagnostics.push(
            Diagnostic::warning(
                codes::LINT_012,
                "summary/long",
                format!(
                    "package.summary is {} characters; RPM summaries should stay under 80",
                    summary.chars().count()
                ),
            )
            .at("package.summary"),
        );
    }

    if recipe.package.source_date_epoch.is_none() {
        diagnostics.push(Diagnostic::warning(
            codes::LINT_013,
            "reproducibility/source-date-epoch",
            "package.source-date-epoch is not set; builds default to timestamp 0",
        ));
    }

    if recipe.package.maintainers.is_empty() {
        diagnostics.push(Diagnostic::warning(
            codes::LINT_019,
            "metadata/maintainers",
            "package.maintainers is empty; nobody is accountable for this package",
        ));
    }

    for cell in &plan.cells {
        if cell
            .release
            .split(['.', '+'])
            .next()
            .is_some_and(|release| release == "0")
        {
            diagnostics.push(
                Diagnostic::error(
                    codes::LINT_011,
                    "release/zero",
                    format!(
                        "cell '{}' has release '{}'; releases start at 1",
                        cell.id, cell.release
                    ),
                )
                .at("package.release"),
            );
        }
    }

    for (scope, values) in [
        ("build.dependencies.tools", &recipe.build.dependencies.tools),
        ("build.dependencies.host", &recipe.build.dependencies.host),
        (
            "build.dependencies.target",
            &recipe.build.dependencies.target,
        ),
        ("build.dependencies.tests", &recipe.build.dependencies.tests),
    ] {
        let mut seen = BTreeSet::new();
        for value in values {
            if !seen.insert(value) {
                diagnostics.push(
                    Diagnostic::warning(
                        codes::LINT_014,
                        "dependencies/duplicate",
                        format!("{scope} lists '{value}' more than once"),
                    )
                    .at(scope),
                );
            }
        }
    }

    for (kind, names) in [
        (
            "sources",
            recipe
                .sources
                .iter()
                .map(|source| source.name.clone())
                .collect::<Vec<_>>(),
        ),
        (
            "outputs",
            recipe
                .outputs
                .iter()
                .map(|output| output.name.clone())
                .collect::<Vec<_>>(),
        ),
        (
            "tests",
            recipe
                .tests
                .iter()
                .map(|test| test.name.clone())
                .collect::<Vec<_>>(),
        ),
    ] {
        let mut seen = BTreeSet::new();
        for name in names {
            if !seen.insert(name.clone()) {
                diagnostics.push(
                    Diagnostic::error(
                        codes::LINT_015,
                        "names/duplicate",
                        format!("{kind} declares '{name}' more than once"),
                    )
                    .at(kind),
                );
            }
        }
    }

    for patch in &recipe.patches {
        if !recipe_dir.join(&patch.file).is_file() {
            diagnostics.push(
                Diagnostic::error(
                    codes::LINT_017,
                    "patches/missing",
                    format!(
                        "patch '{}' does not exist relative to the recipe",
                        patch.file
                    ),
                )
                .at("patches"),
            );
        }
    }

    for source in &recipe.sources {
        for locator in &source.locators {
            if locator.kind == "http" {
                diagnostics.push(
                    Diagnostic::warning(
                        codes::LINT_018,
                        "sources/insecure",
                        format!(
                            "source '{}' has an http locator; use https or pin a signature",
                            source.name
                        ),
                    )
                    .at("sources"),
                );
            }
        }
    }

    for test in &recipe.tests {
        let has_argv = test.argv.as_ref().is_some_and(|argv| !argv.is_empty());
        let has_pipeline = test
            .pipeline
            .as_ref()
            .is_some_and(|steps| !steps.is_empty());
        if !has_argv && !has_pipeline {
            diagnostics.push(
                Diagnostic::error(
                    codes::LINT_020,
                    "tests/empty",
                    format!("test '{}' has neither argv nor pipeline", test.name),
                )
                .at("tests"),
            );
        }
    }

    if recipe.build.pipeline.is_empty() {
        diagnostics.push(Diagnostic::warning(
            codes::LINT_021,
            "build/empty",
            "build.pipeline is empty; the package installs nothing unless source-assembly does",
        ));
    }

    let serialized = serde_json::to_string(recipe).unwrap_or_default();
    for name in recipe.vars.keys() {
        if !serialized.contains(&format!("vars.{name}")) {
            diagnostics.push(Diagnostic::warning(
                codes::LINT_016,
                "vars/unused",
                format!("vars.{name} is declared but never referenced"),
            ));
        }
    }

    diagnostics
}

/// A small SPDX expression grammar: atoms, parentheses, AND/OR/WITH.
fn spdx_syntax(expression: &str) -> Result<(), String> {
    let mut depth = 0_i32;
    let mut expect_atom = true;
    for token in expression
        .replace('(', " ( ")
        .replace(')', " ) ")
        .split_whitespace()
    {
        match token {
            "(" => {
                if !expect_atom {
                    return Err("missing operator before '('".to_string());
                }
                depth += 1;
            }
            ")" => {
                if expect_atom {
                    return Err("missing operand before ')'".to_string());
                }
                depth -= 1;
                if depth < 0 {
                    return Err("unbalanced ')'".to_string());
                }
                expect_atom = false;
            }
            "AND" | "OR" | "WITH" => {
                if expect_atom {
                    return Err(format!("'{token}' has no left operand"));
                }
                expect_atom = true;
            }
            atom => {
                if atom.is_empty()
                    || atom.eq_ignore_ascii_case("and")
                    || atom.eq_ignore_ascii_case("or")
                {
                    return Err(format!("'{atom}' is not a valid license atom or operator"));
                }
                if !expect_atom {
                    return Err("missing AND/OR between license terms".to_string());
                }
                expect_atom = false;
            }
        }
    }
    if depth != 0 {
        return Err("unbalanced '('".to_string());
    }
    if expect_atom {
        return Err("expression ends with an operator".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check, spdx_syntax};
    use rpmx_core::error::codes;

    const RECIPE: &str = r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: 1
  summary: A probe package
  license: MIT
  source-date-epoch: 1704067200
  maintainers:
    - name: A Maintainer
      email: a@example.org
vars:
  used: "${package.version}"
  unused: "x"
build:
  dependencies:
    tools: [gcc, gcc]
  pipeline:
    - runs: "echo ${vars.used}"
outputs:
  - name: probe
    metapackage: true
tests:
  - name: broken
    stage: build
"#;

    fn diagnostics(recipe: &str) -> Vec<super::Diagnostic> {
        let recipe = rpmx_core::parse_recipe(recipe).expect("recipe");
        let plan = rpmx_core::plan_with_options(
            &recipe,
            rpmx_core::PlanOptions {
                arch: "x86_64",
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan");
        check(&recipe, &plan, std::path::Path::new("package.yaml"))
    }

    #[test]
    fn spdx_accepts_real_expressions_and_rejects_broken_ones() {
        for expression in [
            "MIT",
            "MIT OR Apache-2.0",
            "(MIT OR Apache-2.0) AND BSD-3-Clause",
            "GPL-2.0-only WITH Classpath-exception-2.0",
        ] {
            assert!(spdx_syntax(expression).is_ok(), "{expression}");
        }
        for expression in ["", "MIT OR", "MIT Apache-2.0", "(MIT", "MIT)", "AND MIT"] {
            assert!(spdx_syntax(expression).is_err(), "{expression}");
        }
    }

    #[test]
    fn semantic_rules_fire_together() {
        let diagnostics = diagnostics(RECIPE);
        let codes: Vec<_> = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect();
        assert!(codes.contains(&codes::LINT_014.as_str()), "{codes:?}");
        assert!(codes.contains(&codes::LINT_016.as_str()), "{codes:?}");
        assert!(codes.contains(&codes::LINT_020.as_str()), "{codes:?}");
    }

    #[test]
    fn a_clean_recipe_has_no_diagnostics() {
        let clean = RECIPE
            .replace("tools: [gcc, gcc]", "tools: [gcc]")
            .replace("  unused: \"x\"\n", "")
            .replace(
                "tests:\n  - name: broken\n    stage: build\n",
                "tests:\n  - name: smoke\n    stage: build\n    argv: [\"true\"]\n",
            );
        let diagnostics = diagnostics(&clean);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}
