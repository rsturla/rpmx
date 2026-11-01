use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::error::{codes, Error, Result};
use crate::matrix::{self, Cell};
use crate::model::{Output, Recipe, SourceAssembly, Step, Test};
use crate::steps;
use crate::template;

mod validate;

use validate::{
    validate_cpe, validate_file_metadata, validate_outputs, validate_rpm_component,
    validate_schema, validate_sources, validate_step, validate_steps, validate_tool,
};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub schema: u32,
    pub arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dist_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_release: Option<u32>,
    pub source_assembly: Option<PlannedSourceAssembly>,
    pub cells: Vec<PlannedCell>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedSourceAssembly {
    pub steps: Vec<PlannedStep>,
    pub tools: Vec<PlannedTool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedTool {
    pub name: String,
    pub version: String,
    pub digest: String,
    pub image: Option<String>,
    pub entrypoint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedCell {
    pub id: String,
    pub slug: String,
    pub version: String,
    pub release: String,
    pub package: String,
    pub tools: Vec<String>,
    pub host: Vec<String>,
    pub target: Vec<String>,
    pub test_tools: Vec<String>,
    pub pipeline: Vec<PlannedStep>,
    pub outputs: Vec<PlannedOutput>,
    pub tests: Vec<PlannedTest>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedStep {
    pub uses: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub needs: Vec<String>,
    pub tool_digest: Option<String>,
    pub step_digest: Option<String>,
    pub builtin: bool,
    pub inputs: BTreeMap<String, Value>,
    pub input: Option<String>,
    pub input_file: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedOutput {
    pub name: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub arch: Option<String>,
    pub metapackage: bool,
    pub files: Vec<String>,
    pub exclude: Vec<String>,
    pub license: Vec<String>,
    pub doc: Vec<String>,
    pub config: Vec<String>,
    pub noreplace: Vec<String>,
    pub directories: Vec<crate::model::Directory>,
    pub attributes: Vec<crate::model::Attribute>,
    pub requires: Vec<String>,
    pub recommends: Vec<String>,
    pub suggests: Vec<String>,
    pub supplements: Vec<String>,
    pub enhances: Vec<String>,
    pub provides: Vec<String>,
    pub obsoletes: Vec<String>,
    pub conflicts: Vec<String>,
    pub scriptlets: crate::model::Scriptlets,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedTest {
    pub name: String,
    pub stage: String,
    pub argv: Option<Vec<String>>,
    pub pipeline: Vec<PlannedStep>,
}

#[derive(Debug, Clone, Copy)]
pub struct PlanOptions<'a> {
    pub arch: &'a str,
    pub step_root: Option<&'a std::path::Path>,
    pub allow_builtin_override: bool,
    /// Distribution suffix appended to every release, such as `.fc44`.
    pub dist_tag: Option<&'a str>,
    /// Build-system counter rendered as `+build<N>` after the dist tag.
    pub build_release: Option<u32>,
}

pub fn plan(recipe: &Recipe, arch: &str) -> Result<Plan> {
    plan_with_options(
        recipe,
        PlanOptions {
            arch,
            step_root: None,
            allow_builtin_override: false,
            dist_tag: None,
            build_release: None,
        },
    )
}

/// Normalize a human-supplied dist tag to the canonical `.fc44` form.
///
/// The leading dot is required in the release field but optional on input.
/// `+` is rejected because `+build<N>` is a separate build-system counter.
pub fn normalize_dist_tag(raw: &str) -> Result<String> {
    if raw.is_empty() || raw.trim() != raw {
        return Err(invalid_dist_tag(raw));
    }
    let tag = raw.strip_prefix('.').unwrap_or(raw);
    let valid = tag
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && tag
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_')
        && !tag.contains("..")
        && !tag.ends_with('.');
    if !valid {
        return Err(invalid_dist_tag(raw));
    }
    Ok(format!(".{tag}"))
}

fn invalid_dist_tag(raw: &str) -> Error {
    Error::new(
        codes::PLAN_012,
        format!(
            "dist tag '{raw}' is invalid; expected a value like 'fc44' or '.el9_2' \
             (letters, digits, dots and underscores)"
        ),
    )
    .at("dist-tag")
}

pub fn plan_with_options(recipe: &Recipe, options: PlanOptions<'_>) -> Result<Plan> {
    let arch = options.arch;
    let step_root = options.step_root;
    let allow_builtin_override = options.allow_builtin_override;
    let dist_tag = options.dist_tag.map(normalize_dist_tag).transpose()?;
    let build_release = options.build_release;
    validate_schema(recipe)?;
    validate_cpe(recipe)?;
    validate_steps(recipe)?;
    validate_sources(recipe)?;
    validate_outputs(recipe)?;

    let source_assembly = plan_source_assembly(
        &recipe.source_assembly,
        recipe,
        arch,
        dist_tag.as_deref(),
        step_root,
        allow_builtin_override,
    )?;

    let cells = matrix::expand(recipe.matrix.as_ref())?;
    let release_suffix = recipe
        .matrix
        .as_ref()
        .and_then(|matrix| matrix.release_suffix)
        .unwrap_or(true);

    let mut planned = Vec::with_capacity(cells.len());
    for cell in &cells {
        planned.push(plan_cell(
            recipe,
            cell,
            arch,
            CellPlanOptions {
                release_suffix,
                dist_tag: dist_tag.as_deref(),
                build_release,
                step_root,
                allow_builtin_override,
            },
        )?);
    }

    let mut output_owner: BTreeMap<String, String> = BTreeMap::new();
    let mut srpm_owner: BTreeMap<String, String> = BTreeMap::new();
    for cell in &planned {
        let srpm = format!("{}-{}", cell.package, cell.release);
        if let Some(previous) = srpm_owner.insert(srpm.clone(), cell.id.clone()) {
            return Err(Error::new(
                codes::PLAN_001,
                format!(
                    "cells '{}' and '{}' both produce source package '{}'",
                    previous, cell.id, srpm
                ),
            ));
        }
        for output in &cell.outputs {
            if let Some(previous) = output_owner.insert(output.name.clone(), cell.id.clone()) {
                return Err(Error::new(
                    codes::PLAN_002,
                    format!(
                        "output '{}' is produced by both '{}' and '{}'",
                        output.name, previous, cell.id
                    ),
                ));
            }
        }
    }

    Ok(Plan {
        schema: SCHEMA_VERSION,
        arch: arch.to_string(),
        dist_tag,
        build_release,
        source_assembly,
        cells: planned,
    })
}

fn plan_source_assembly(
    source: &SourceAssembly,
    recipe: &Recipe,
    arch: &str,
    dist_tag: Option<&str>,
    step_root: Option<&std::path::Path>,
    allow_builtin_override: bool,
) -> Result<Option<PlannedSourceAssembly>> {
    if source.pipeline.is_empty() && source.tools.is_empty() {
        return Ok(None);
    }

    let mut scope = base_scope_without_cell(recipe, arch, dist_tag);
    let steps = if source.pipeline.is_empty() {
        Vec::new()
    } else {
        let vars = template::expand_vars(&recipe.vars, &scope)?;
        for (name, value) in &vars {
            scope.insert(format!("vars.{name}"), Value::String(value.clone()));
        }
        expand_steps(&source.pipeline, &scope, "source-assembly.pipeline")?
    };
    for (index, step) in steps.iter().enumerate() {
        validate_step(step, format!("source-assembly.pipeline[{index}]"))?;
    }

    let mut tools = Vec::with_capacity(source.tools.len());
    let mut tool_names = BTreeSet::new();
    for (index, tool) in source.tools.iter().enumerate() {
        validate_tool(tool, index)?;
        if !tool_names.insert(tool.name.clone()) {
            return Err(Error::new(
                codes::TOOL_003,
                format!("tool '{}' is declared twice", tool.name),
            )
            .at(format!("source-assembly.tools[{index}]")));
        }
        let image = tool
            .locators
            .iter()
            .find(|locator| locator.kind == "oci")
            .and_then(|locator| locator.reference.clone());
        tools.push(PlannedTool {
            name: tool.name.clone(),
            version: tool.version.clone(),
            digest: tool.digest.clone(),
            image,
            entrypoint: tool.entrypoint.clone(),
        });
    }

    let planned_steps = plan_steps(
        &steps,
        step_root,
        "source-assembly.pipeline",
        allow_builtin_override,
    )?;

    let last_bump = steps
        .iter()
        .rposition(|step| step.uses.as_deref() == Some("go/bump"));
    let last_vendor = steps
        .iter()
        .rposition(|step| step.uses.as_deref() == Some("go/vendor"));
    if let Some(bump) = last_bump {
        if last_vendor.is_none_or(|vendor| vendor < bump) {
            return Err(Error::new(
                codes::ASSEMBLY_016,
                "go/bump must be followed by a go/vendor step so the SRPM builds offline",
            )
            .at(format!("source-assembly.pipeline[{bump}]")));
        }
    }

    Ok(Some(PlannedSourceAssembly {
        steps: planned_steps,
        tools,
    }))
}

fn plan_steps(
    steps: &[Step],
    step_root: Option<&std::path::Path>,
    context: &str,
    allow_builtin_override: bool,
) -> Result<Vec<PlannedStep>> {
    let mut planned = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let at = |field: &str| format!("{context}[{index}].{field}");

        if let Some(uses) = &step.uses {
            let resolved =
                steps::resolve(uses, step_root, step.with.as_ref(), allow_builtin_override)?;
            planned.push(PlannedStep {
                uses: uses.clone(),
                command: Some(resolved.command),
                args: resolved.args,
                cwd: resolved.cwd,
                env: resolved.env,
                needs: resolved.needs,
                tool_digest: None,
                step_digest: Some(resolved.digest),
                builtin: resolved.builtin,
                inputs: step.with.clone().unwrap_or_default(),
                input: resolved.input,
                input_file: resolved.input_file,
            });
            continue;
        }

        if let Some(argv) = &step.argv {
            let (command, args) = argv.split_first().ok_or_else(|| {
                Error::new(codes::STEP_002, "argv must not be empty").at(at("argv"))
            })?;
            planned.push(PlannedStep {
                uses: step.label(),
                command: Some(command.clone()),
                args: args.to_vec(),
                cwd: step.cwd.clone(),
                env: step.env.clone().unwrap_or_default(),
                needs: Vec::new(),
                tool_digest: None,
                step_digest: None,
                builtin: false,
                inputs: BTreeMap::new(),
                input: None,
                input_file: None,
            });
            continue;
        }

        let runs = step
            .runs
            .as_ref()
            .ok_or_else(|| Error::new(codes::STEP_001, "step has no command").at(at("runs")))?;
        planned.push(PlannedStep {
            uses: step.label(),
            command: Some("sh".to_string()),
            args: vec!["-c".to_string(), runs.clone()],
            cwd: step.cwd.clone(),
            env: step.env.clone().unwrap_or_default(),
            needs: Vec::new(),
            tool_digest: None,
            step_digest: None,
            builtin: false,
            inputs: BTreeMap::new(),
            input: None,
            input_file: None,
        });
    }
    Ok(planned)
}

#[derive(Clone, Copy)]
struct CellPlanOptions<'a> {
    release_suffix: bool,
    dist_tag: Option<&'a str>,
    build_release: Option<u32>,
    step_root: Option<&'a std::path::Path>,
    allow_builtin_override: bool,
}

fn plan_cell(
    recipe: &Recipe,
    cell: &Cell,
    arch: &str,
    options: CellPlanOptions<'_>,
) -> Result<PlannedCell> {
    let mut scope = base_scope(recipe, cell, arch, options.dist_tag);
    let vars = template::expand_vars(&recipe.vars, &scope)?;
    for (name, value) in &vars {
        scope.insert(format!("vars.{name}"), Value::String(value.clone()));
    }

    let package = template::expand_string(&recipe.package.name, &scope)
        .map_err(|error| error.at("package.name"))?;
    let version = template::expand_string(&recipe.package.version, &scope)
        .map_err(|error| error.at("package.version"))?;
    if package.trim().is_empty() {
        return Err(
            Error::new(codes::PLAN_003, "package.name expanded to an empty string")
                .at("package.name"),
        );
    }
    validate_rpm_component("package.name", &package)?;

    let mut release = template::expand_string(&recipe.package.release, &scope)
        .map_err(|error| error.at("package.release"))?;
    // A recipe may spell the policy suffixes out through templates. Strip them
    // before the matrix suffix and add them back in canonical order, so the
    // final release never contains a suffix twice or in the wrong position.
    // Remove at most one copy of each policy suffix, accepting either order
    // (`tag+build` or `build+tag`). Stripping repeatedly would eat a base
    // release that legitimately ends with the same text, such as tag `1` on
    // release `2.1.1`.
    let build_suffix = options
        .build_release
        .map(|number| format!("+build{number}"));
    match (options.dist_tag, build_suffix.as_deref()) {
        (Some(tag), Some(build)) => {
            let both = [format!("{tag}{build}"), format!("{build}{tag}")];
            if let Some(stripped) = both.iter().find_map(|suffix| release.strip_suffix(suffix)) {
                release = stripped.to_string();
            } else if let Some(stripped) = release
                .strip_suffix(tag)
                .or_else(|| release.strip_suffix(build))
            {
                release = stripped.to_string();
            }
        }
        (Some(tag), None) => {
            if let Some(stripped) = release.strip_suffix(tag) {
                release = stripped.to_string();
            }
        }
        (None, Some(build)) => {
            if let Some(stripped) = release.strip_suffix(build) {
                release = stripped.to_string();
            }
        }
        (None, None) => {}
    }
    if release.trim().is_empty() {
        return Err(Error::new(
            codes::PLAN_004,
            "package.release expanded to an empty string",
        )
        .at("package.release"));
    }

    let release_already_has_slug = recipe.package.release.contains("${cell.slug}");
    if options.release_suffix && !cell.slug.is_empty() && !release_already_has_slug {
        release = format!("{release}.{}", cell.slug);
    }
    if let Some(tag) = options.dist_tag {
        release.push_str(tag);
    }
    if let Some(number) = options.build_release {
        release.push_str(&format!("+build{number}"));
    }
    validate_rpm_component("package.version", &version)?;
    validate_rpm_component("package.release", &release)?;

    scope.insert("package.name".to_string(), Value::String(package.clone()));
    scope.insert(
        "package.version".to_string(),
        Value::String(version.clone()),
    );
    scope.insert(
        "package.release".to_string(),
        Value::String(release.clone()),
    );

    let tools = expand_dependencies(&recipe.build.dependencies.tools, &scope, "tools")?;
    let host = expand_dependencies(&recipe.build.dependencies.host, &scope, "host")?;
    let target = expand_dependencies(&recipe.build.dependencies.target, &scope, "target")?;
    let test_tools = expand_dependencies(&recipe.build.dependencies.tests, &scope, "tests")?;

    let pipeline = expand_pipeline(&recipe.build.pipeline, &scope)?;
    let mut pipeline = plan_steps(
        &pipeline,
        options.step_root,
        "build.pipeline",
        options.allow_builtin_override,
    )?;
    for step in &mut pipeline {
        for (key, value) in &recipe.build.environment {
            let value = template::expand_string(value, &scope)?;
            step.env.entry(key.clone()).or_insert(value);
        }
    }
    let outputs = expand_outputs(&recipe.outputs, &scope)?;
    let tests = expand_tests(
        &recipe.tests,
        &scope,
        options.step_root,
        options.allow_builtin_override,
    )?;

    Ok(PlannedCell {
        id: cell.id.clone(),
        slug: cell.slug.clone(),
        version,
        release,
        package,
        tools,
        host,
        target,
        test_tools,
        pipeline,
        outputs,
        tests,
    })
}

fn expand_dependencies(
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

fn base_scope(
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

fn base_scope_without_cell(
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
        Value::String(crate::guest::BUILD.into()),
    );
    scope.insert(
        "build.dir".into(),
        Value::String(crate::guest::BUILD.into()),
    );
    scope.insert("stage".into(), Value::String(crate::guest::STAGE.into()));
    scope.insert("prefix".into(), Value::String(crate::guest::PREFIX.into()));
    scope.insert("libdir".into(), Value::String(crate::guest::LIBDIR.into()));
    scope.insert("jobs".into(), Value::String("1".into()));
    scope
}

fn expand_pipeline(steps: &[Step], scope: &BTreeMap<String, Value>) -> Result<Vec<Step>> {
    expand_steps(steps, scope, "build.pipeline")
}

fn expand_steps(steps: &[Step], scope: &BTreeMap<String, Value>, path: &str) -> Result<Vec<Step>> {
    let raw = serde_json::to_value(steps)
        .map_err(|error| Error::new(codes::PLAN_009, error.to_string()))?;
    let expanded = template::expand_json(&raw, scope).map_err(|error| error.at(path))?;
    serde_json::from_value(expanded)
        .map_err(|error| Error::new(codes::PLAN_010, error.to_string()).at(path))
}

fn expand_outputs(
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

fn expand_tests(
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
