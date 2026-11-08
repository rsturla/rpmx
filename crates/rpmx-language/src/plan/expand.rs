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
    expand_values(values, scope, &format!("build.dependencies.{field}"))
}

pub(crate) fn expand_values(
    values: &[String],
    scope: &BTreeMap<String, Value>,
    field: &str,
) -> Result<Vec<String>> {
    values
        .iter()
        .map(|value| template::expand_string(value, scope).map_err(|error| error.at(field)))
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

pub(crate) struct ExpandedOutputs {
    pub outputs: Vec<PlannedOutput>,
    pub tests: Vec<PlannedTest>,
}

pub(crate) fn expand_outputs(
    outputs: &[Output],
    scope: &BTreeMap<String, Value>,
    step_root: Option<&std::path::Path>,
    allow_builtin_override: bool,
) -> Result<ExpandedOutputs> {
    let mut planned = Vec::with_capacity(outputs.len());
    let mut tests = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, output) in outputs.iter().enumerate() {
        // Expand the output itself without its tests, then give each test the
        // output's expanded name as its owner. Expanding the whole output at
        // once would expand test values twice and lose the per-test scope.
        let mut raw = output.clone();
        let output_tests = std::mem::take(&mut raw.tests);
        let mut output: Output = expand_typed(
            &raw,
            scope,
            &format!("outputs[{index}]"),
            codes::PLAN_005,
            codes::PLAN_006,
        )?;
        expand_file_presets(
            &mut output.files.include,
            &format!("outputs[{index}].files.include"),
        )?;
        expand_file_presets(
            &mut output.files.exclude,
            &format!("outputs[{index}].files.exclude"),
        )?;
        if output.name.trim().is_empty() {
            return Err(
                Error::new(codes::OUTPUT_004, "output name expanded to an empty string")
                    .at(format!("outputs[{index}].name")),
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
        tests.extend(expand_tests(
            &output_tests,
            scope,
            step_root,
            allow_builtin_override,
            Some(&output.name),
            &format!("outputs[{index}].tests"),
        )?);
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
            lint: output.lint,
            scriptlets: output.scriptlets,
        });
    }
    Ok(ExpandedOutputs {
        outputs: planned,
        tests,
    })
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

fn file_preset(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "@dev" => Some(&[
            "/usr/include/**",
            "/usr/lib*/lib*.so",
            "/usr/lib*/pkgconfig/**",
            "/usr/share/pkgconfig/**",
            "/usr/share/aclocal/**",
            "/usr/lib*/cmake/**",
            "/usr/share/cmake/**",
        ]),
        "@headers" => Some(&["/usr/include/**"]),
        "@static" => Some(&["/usr/lib*/lib*.a"]),
        "@pkgconfig" => Some(&["/usr/lib*/pkgconfig/**", "/usr/share/pkgconfig/**"]),
        "@doc" => Some(&["/usr/share/doc/**"]),
        "@man" => Some(&["/usr/share/man/**"]),
        "@info" => Some(&["/usr/share/info/**"]),
        "@locales" => Some(&["/usr/share/i18n/**", "/usr/share/locale/**"]),
        "@debug" => Some(&["/usr/lib/debug/**", "/usr/lib*/*.debug"]),
        _ => None,
    }
}

fn expand_file_presets(patterns: &mut Vec<String>, path: &str) -> Result<()> {
    let mut expanded = Vec::with_capacity(patterns.len());
    for pattern in patterns.drain(..) {
        if let Some(preset) = file_preset(&pattern) {
            expanded.extend(preset.iter().map(|value| (*value).to_string()));
            continue;
        }
        if pattern.starts_with('@') {
            return Err(Error::new(
                codes::OUTPUT_019,
                format!(
                    "unknown file preset '{pattern}'; known presets: @dev, @headers, @static, \
                     @pkgconfig, @doc, @man, @info, @locales, @debug"
                ),
            )
            .at(path));
        }
        expanded.push(pattern);
    }
    *patterns = expanded;
    Ok(())
}

pub(crate) fn expand_tests(
    tests: &[Test],
    scope: &BTreeMap<String, Value>,
    step_root: Option<&std::path::Path>,
    allow_builtin_override: bool,
    owner: Option<&str>,
    path: &str,
) -> Result<Vec<PlannedTest>> {
    let tests: Vec<Test> = expand_typed_deferring(
        &tests.to_vec(),
        scope,
        path,
        codes::PLAN_007,
        codes::PLAN_008,
    )?;

    let mut seen = BTreeSet::new();
    let mut planned = Vec::with_capacity(tests.len());
    for (index, test) in tests.into_iter().enumerate() {
        let at = format!("{path}[{index}]");
        if test.name.contains("${") || test.stage.contains("${") {
            return Err(Error::new(
                codes::TEMPLATE_007,
                "a test name or stage cannot contain an executor placeholder",
            )
            .at(at));
        }
        if test.name.trim().is_empty() {
            return Err(
                Error::new(codes::TEST_001, "test name expanded to an empty string")
                    .at(format!("{path}[].name")),
            );
        }
        if test.name.contains('/') {
            return Err(Error::new(
                codes::TEST_001,
                format!(
                    "test name '{}' cannot contain '/'; that separator selects output-level tests",
                    test.name
                ),
            )
            .at(format!("{path}[{index}].name")));
        }
        if !seen.insert(test.name.clone()) {
            let owner = owner
                .map(|output| format!(" in output '{output}'"))
                .unwrap_or_default();
            return Err(Error::new(
                codes::TEST_002,
                format!("test '{}' is declared twice{owner}", test.name),
            ));
        }
        let name = test.name;
        let packages = expand_values(
            &test.environment.contents.packages,
            scope,
            &format!("{path}[{index}].environment.contents.packages"),
        )?;
        if test.stage != "installed" && !packages.is_empty() {
            return Err(Error::new(
                codes::TEST_004,
                format!(
                    "test '{name}' declares environment.contents.packages but uses stage '{}'; \
                     test packages are installed into the disposable installed-stage root",
                    test.stage
                ),
            )
            .at(format!("{path}[{index}].environment.contents.packages")));
        }
        let environment = test
            .environment
            .environment
            .iter()
            .map(|(key, value)| {
                template::expand_string(value, scope)
                    .map(|value| (key.clone(), value))
                    .map_err(|error| error.at(format!("{path}[{index}].environment.environment")))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let step_scope = owner
            .map(|output| format!("test:{output}/{name}"))
            .unwrap_or_else(|| format!("test:{name}"));
        planned.push(PlannedTest {
            name,
            output: owner.map(str::to_string),
            stage: test.stage,
            argv: test.argv,
            pipeline: match test.pipeline {
                Some(pipeline) => plan_steps(
                    &pipeline,
                    step_root,
                    &format!("{path}[{index}].pipeline"),
                    allow_builtin_override,
                    &step_scope,
                )?,
                None => Vec::new(),
            },
            packages,
            environment,
        });
    }
    Ok(planned)
}

#[cfg(test)]
mod tests {
    use super::file_preset;

    #[test]
    fn the_dev_preset_does_not_claim_static_archives() {
        let dev = file_preset("@dev").expect("@dev");
        let static_libs = file_preset("@static").expect("@static");
        assert!(!dev.contains(&"/usr/lib*/lib*.a"), "{dev:?}");
        assert!(static_libs.contains(&"/usr/lib*/lib*.a"), "{static_libs:?}");
    }
}
