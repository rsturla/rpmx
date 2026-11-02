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
    scope
}

pub(crate) fn expand_pipeline(
    steps: &[Step],
    scope: &BTreeMap<String, Value>,
) -> Result<Vec<Step>> {
    expand_steps(steps, scope, "build.pipeline")
}

fn expand_typed<T: serde::Serialize + serde::de::DeserializeOwned>(
    value: &T,
    scope: &BTreeMap<String, Value>,
    path: &str,
    encode_code: rpmx_base::error::Code,
    decode_code: rpmx_base::error::Code,
) -> Result<T> {
    let raw =
        serde_json::to_value(value).map_err(|error| Error::new(encode_code, error.to_string()))?;
    let expanded = template::expand_json(&raw, scope).map_err(|error| error.at(path))?;
    serde_json::from_value(expanded)
        .map_err(|error| Error::new(decode_code, error.to_string()).at(path))
}

fn expand_typed_deferring<T: serde::Serialize + serde::de::DeserializeOwned>(
    value: &T,
    scope: &BTreeMap<String, Value>,
    path: &str,
    encode_code: rpmx_base::error::Code,
    decode_code: rpmx_base::error::Code,
) -> Result<T> {
    let raw =
        serde_json::to_value(value).map_err(|error| Error::new(encode_code, error.to_string()))?;
    let expanded = template::expand_json_deferring(&raw, scope).map_err(|error| error.at(path))?;
    serde_json::from_value(expanded)
        .map_err(|error| Error::new(decode_code, error.to_string()).at(path))
}

pub(crate) fn expand_steps(
    steps: &[Step],
    scope: &BTreeMap<String, Value>,
    path: &str,
) -> Result<Vec<Step>> {
    expand_typed_deferring(
        &steps.to_vec(),
        scope,
        path,
        codes::PLAN_009,
        codes::PLAN_010,
    )
}

pub(crate) fn expand_outputs(
    outputs: &[Output],
    scope: &BTreeMap<String, Value>,
) -> Result<Vec<PlannedOutput>> {
    let outputs: Vec<Output> = expand_typed(
        &outputs.to_vec(),
        scope,
        "outputs",
        codes::PLAN_005,
        codes::PLAN_006,
    )?;

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
        validate_bundled(&output)?;
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
            dependencies: output.dependencies.to_set(),
            bundled: output.bundled,
            generators: output.generators,
            scriptlets: output.scriptlets,
        });
    }
    Ok(planned)
}

fn validate_bundled(output: &Output) -> Result<()> {
    let mut seen = BTreeSet::new();
    for declaration in &output.bundled {
        let name = declaration.name.trim();
        let valid_name = name == declaration.name
            && !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphanumeric())
            && name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '_' | '-'));
        if !valid_name {
            return Err(Error::new(
                codes::OUTPUT_004,
                format!(
                    "output '{}' has invalid bundled name '{}'",
                    output.name, declaration.name
                ),
            ));
        }
        if !seen.insert(name.to_string()) {
            return Err(Error::new(
                codes::OUTPUT_005,
                format!(
                    "output '{}' declares bundled component '{}' twice",
                    output.name, name
                ),
            ));
        }
        if let Some(version) = &declaration.version {
            if version.is_empty()
                || !version.is_ascii()
                || version.chars().any(|ch| {
                    ch.is_whitespace()
                        || ch.is_control()
                        || matches!(ch, '=' | '(' | ')' | '/' | ':' | '-' | '[' | ']')
                })
            {
                return Err(Error::new(
                    codes::OUTPUT_004,
                    format!(
                        "output '{}' has invalid version '{}' for bundled component '{}'",
                        output.name, version, name
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn expand_tests(
    tests: &[Test],
    scope: &BTreeMap<String, Value>,
    step_root: Option<&std::path::Path>,
    allow_builtin_override: bool,
) -> Result<Vec<PlannedTest>> {
    let tests: Vec<Test> = expand_typed_deferring(
        &tests.to_vec(),
        scope,
        "tests",
        codes::PLAN_007,
        codes::PLAN_008,
    )?;

    let mut seen = BTreeSet::new();
    let mut planned = Vec::with_capacity(tests.len());
    for (index, test) in tests.into_iter().enumerate() {
        if test.name.contains("${") || test.stage.contains("${") {
            return Err(Error::new(
                codes::TEMPLATE_007,
                "a test name or stage cannot contain an executor placeholder",
            )
            .at(format!("tests[{index}]")));
        }
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
        let name = test.name;
        let scope = format!("test:{name}");
        planned.push(PlannedTest {
            name,
            stage: test.stage,
            argv: test.argv,
            pipeline: match test.pipeline {
                Some(pipeline) => plan_steps(
                    &pipeline,
                    step_root,
                    &format!("tests[{index}].pipeline"),
                    allow_builtin_override,
                    &scope,
                )?,
                None => Vec::new(),
            },
        });
    }
    Ok(planned)
}
