use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::matrix::Cell;
use crate::model::{Output, Recipe, Step, Test};
use crate::template;
use rpmx_base::error::{codes, Error, Result};

use super::validate::validate_file_metadata;
use super::{plan_steps, PlannedOutput, PlannedTest};

pub(crate) fn expand_dependencies(
    values: &[String],
    scope: &BTreeMap<String, Value>,
    field: &str,
) -> Result<Vec<String>> {
    values
        .iter()
        .map(|value| {
            template::expand_string(value, scope)
                .map_err(|error| error.at(format!("build.dependencies.{field}")))
        })
        .collect()
}

pub(crate) fn base_scope(
    recipe: &Recipe,
    cell: &Cell,
    arch: &str,
    dist_tag: Option<&str>,
) -> BTreeMap<String, Value> {
    let mut scope = base_scope_without_cell(recipe, arch, dist_tag);

    for (axis, value) in &cell.identity {
        scope.insert(format!("matrix.{axis}"), value.clone());
    }
    for (axis, fields) in &cell.fields {
        for (field, value) in fields {
            scope.insert(format!("matrix.{axis}.{field}"), value.clone());
        }
    }

    scope.insert("cell.id".into(), Value::String(cell.id.clone()));
    scope.insert("cell.slug".into(), Value::String(cell.slug.clone()));
    scope
}

pub(crate) fn base_scope_without_cell(
    recipe: &Recipe,
    arch: &str,
    dist_tag: Option<&str>,
) -> BTreeMap<String, Value> {
    let mut scope = BTreeMap::new();
    scope.insert(
        "profile.dist_tag".into(),
        Value::String(dist_tag.unwrap_or_default().to_string()),
    );
    scope.insert(
        "package.name".into(),
        Value::String(recipe.package.name.clone()),
    );
    scope.insert(
        "package.version".into(),
        Value::String(recipe.package.version.clone()),
    );
    scope.insert(
        "package.release".into(),
        Value::String(recipe.package.release.clone()),
    );
    scope.insert(
        "package.epoch".into(),
        Value::Number(recipe.package.epoch.unwrap_or(0).into()),
    );
    scope.insert(
        "package.license".into(),
        Value::String(recipe.package.license.clone()),
    );
    scope.insert("arch".into(), Value::String(arch.to_string()));
    scope.insert(
        "source.dir".into(),
        Value::String(rpmx_base::guest::BUILD.into()),
    );
    scope.insert(
        "build.dir".into(),
        Value::String(rpmx_base::guest::BUILD.into()),
    );
    scope.insert(
        "stage".into(),
        Value::String(rpmx_base::guest::STAGE.into()),
    );
    scope.insert(
        "prefix".into(),
        Value::String(rpmx_base::guest::PREFIX.into()),
    );
    scope.insert(
        "libdir".into(),
        Value::String(rpmx_base::guest::LIBDIR.into()),
    );
    scope.insert("jobs".into(), Value::String("1".into()));
    scope
}

pub(crate) fn expand_pipeline(
    steps: &[Step],
    scope: &BTreeMap<String, Value>,
) -> Result<Vec<Step>> {
    expand_steps(steps, scope, "build.pipeline")
}

pub(crate) fn expand_steps(
    steps: &[Step],
    scope: &BTreeMap<String, Value>,
    path: &str,
) -> Result<Vec<Step>> {
    let raw = serde_json::to_value(steps)
        .map_err(|error| Error::new(codes::PLAN_009, error.to_string()))?;
    let expanded = template::expand_json(&raw, scope).map_err(|error| error.at(path))?;
    serde_json::from_value(expanded)
        .map_err(|error| Error::new(codes::PLAN_010, error.to_string()).at(path))
}

pub(crate) fn expand_outputs(
    outputs: &[Output],
    scope: &BTreeMap<String, Value>,
) -> Result<Vec<PlannedOutput>> {
    let raw = serde_json::to_value(outputs)
        .map_err(|error| Error::new(codes::PLAN_005, error.to_string()))?;
    let expanded = template::expand_json(&raw, scope).map_err(|error| error.at("outputs"))?;
    let outputs: Vec<Output> = serde_json::from_value(expanded)
        .map_err(|error| Error::new(codes::PLAN_006, error.to_string()).at("outputs"))?;

    let mut planned = Vec::with_capacity(outputs.len());
    let mut seen = BTreeSet::new();
    for output in outputs {
        if output.name.trim().is_empty() {
            return Err(
                Error::new(codes::OUTPUT_004, "output name expanded to an empty string")
                    .at("outputs[].name"),
            );
        }
        if !seen.insert(output.name.clone()) {
            return Err(Error::new(
                codes::OUTPUT_005,
                format!("output '{}' is declared twice in one cell", output.name),
            ));
        }
        validate_file_metadata(&output)?;
        planned.push(PlannedOutput {
            name: output.name,
            summary: output.summary,
            description: output.description,
            arch: output.arch,
            metapackage: output.metapackage,
            files: output.files.include,
            exclude: output.files.exclude,
            license: output.files.license,
            doc: output.files.doc,
            config: output.files.config,
            noreplace: output.files.noreplace,
            directories: output.files.directories,
            attributes: output.files.attributes,
            requires: output.dependencies.run,
            recommends: output.dependencies.recommends,
            suggests: output.dependencies.suggests,
            supplements: output.dependencies.supplements,
            enhances: output.dependencies.enhances,
            provides: output.dependencies.provides,
            obsoletes: output.dependencies.obsoletes,
            conflicts: output.dependencies.conflicts,
            scriptlets: output.scriptlets,
        });
    }
    Ok(planned)
}

pub(crate) fn expand_tests(
    tests: &[Test],
    scope: &BTreeMap<String, Value>,
    step_root: Option<&std::path::Path>,
    allow_builtin_override: bool,
) -> Result<Vec<PlannedTest>> {
    let raw = serde_json::to_value(tests)
        .map_err(|error| Error::new(codes::PLAN_007, error.to_string()))?;
    let expanded = template::expand_json(&raw, scope).map_err(|error| error.at("tests"))?;
    let tests: Vec<Test> = serde_json::from_value(expanded)
        .map_err(|error| Error::new(codes::PLAN_008, error.to_string()).at("tests"))?;

    let mut seen = BTreeSet::new();
    let mut planned = Vec::with_capacity(tests.len());
    for (index, test) in tests.into_iter().enumerate() {
        if test.name.trim().is_empty() {
            return Err(
                Error::new(codes::TEST_001, "test name expanded to an empty string")
                    .at("tests[].name"),
            );
        }
        if !seen.insert(test.name.clone()) {
            return Err(Error::new(
                codes::TEST_002,
                format!("test '{}' is declared twice in one cell", test.name),
            ));
        }
        planned.push(PlannedTest {
            name: test.name,
            stage: test.stage,
            argv: test.argv,
            pipeline: match test.pipeline {
                Some(pipeline) => plan_steps(
                    &pipeline,
                    step_root,
                    &format!("tests[{index}].pipeline"),
                    allow_builtin_override,
                )?,
                None => Vec::new(),
            },
        });
    }
    Ok(planned)
}
