use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::matrix::{self, Cell};
use crate::model::{Recipe, SourceAssembly, Step};
use crate::steps;
use crate::template;
use rpmx_base::error::{codes, Error, Result};

mod expand;

use expand::{
    base_scope, base_scope_without_cell, expand_dependencies, expand_outputs, expand_pipeline,
    expand_steps, expand_tests, ExpandedOutputs,
};

mod validate;

use validate::{
    validate_cpe, validate_outputs, validate_rpm_component, validate_schema, validate_sources,
    validate_step, validate_steps, validate_tool,
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
    /// Stable display identity. Explicit `name`, then `uses`, then a stable
    /// ordinal such as `runs#2`; never derived from script content.
    pub label: String,
    pub uses: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    /// Union of `needs_tools` and `needs_host`, for native builds.
    pub needs: Vec<String>,
    /// Packages the build root needs: the compiler, generators, `make`.
    pub needs_tools: Vec<String>,
    /// Packages the target sysroot needs: headers and libraries.
    pub needs_host: Vec<String>,
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
    #[serde(flatten)]
    pub dependencies: crate::model::DependencySet,
    pub bundled: Vec<crate::model::BundledDeclaration>,
    pub generators: crate::model::OutputGenerators,
    pub scriptlets: crate::model::Scriptlets,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedTest {
    pub name: String,
    /// Output that owns this test. `None` is a recipe-level test, which sees
    /// every output; `Some` is an output-level test, which sees that output
    /// and its runtime dependencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub stage: String,
    pub argv: Option<Vec<String>>,
    pub pipeline: Vec<PlannedStep>,
    /// Packages that are installed only for this test's root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    /// Variables applied to every step of this test.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
}

impl PlannedTest {
    /// The name users type for `--test`. An output-level test keeps its bare
    /// name usable when it is unique, and can always be selected as
    /// `<output>/<name>`.
    pub fn qualified_name(&self) -> String {
        match &self.output {
            Some(output) => format!("{output}/{}", self.name),
            None => self.name.clone(),
        }
    }

    /// Match either the bare test name or `<output>/<name>`.
    pub fn matches_selector(&self, selector: &str) -> bool {
        if selector == self.name {
            return true;
        }
        let Some(output) = &self.output else {
            return false;
        };
        selector
            .strip_prefix(output.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
            == Some(self.name.as_str())
    }
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
        "source",
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

/// Assign a stable, unique display label to every step in one pipeline.
///
/// An explicit `name` wins, then `uses`, then the first `argv` word. An
/// anonymous `runs:` step is numbered by its position among anonymous steps,
/// never by its script text, so two identical scripts still get distinct
/// labels.
fn step_labels(steps: &[Step], scope: &str) -> Vec<String> {
    let mut used = BTreeSet::new();
    let mut runs = 0usize;
    steps
        .iter()
        .map(|step| {
            let base = if let Some(name) = &step.name {
                name.clone()
            } else if let Some(uses) = &step.uses {
                uses.clone()
            } else if let Some(argv) = &step.argv {
                argv.first().cloned().unwrap_or_else(|| "argv".to_string())
            } else {
                runs += 1;
                format!("{scope}:runs#{runs}")
            };
            let mut label = base.clone();
            let mut suffix = 1usize;
            while !used.insert(label.clone()) {
                suffix += 1;
                label = format!("{base}#{suffix}");
            }
            label
        })
        .collect()
}

pub(crate) fn plan_steps(
    steps: &[Step],
    step_root: Option<&std::path::Path>,
    context: &str,
    allow_builtin_override: bool,
    scope: &str,
) -> Result<Vec<PlannedStep>> {
    let labels = step_labels(steps, scope);
    let mut planned = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let at = |field: &str| format!("{context}[{index}].{field}");
        let label = labels[index].clone();

        if let Some(uses) = &step.uses {
            let resolved =
                steps::resolve(uses, step_root, step.with.as_ref(), allow_builtin_override)?;
            planned.push(PlannedStep {
                label,
                uses: uses.clone(),
                command: Some(resolved.command),
                args: resolved.args,
                cwd: resolved.cwd,
                env: resolved.env,
                needs: resolved.needs,
                needs_tools: resolved.needs_tools,
                needs_host: resolved.needs_host,
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
                label,
                uses: step.label(),
                command: Some(command.clone()),
                args: args.to_vec(),
                cwd: step.cwd.clone(),
                env: step.env.clone().unwrap_or_default(),
                needs: Vec::new(),
                needs_tools: Vec::new(),
                needs_host: Vec::new(),
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
            label,
            uses: step.label(),
            command: Some("sh".to_string()),
            args: vec!["-c".to_string(), runs.clone()],
            cwd: step.cwd.clone(),
            env: step.env.clone().unwrap_or_default(),
            needs: Vec::new(),
            needs_tools: Vec::new(),
            needs_host: Vec::new(),
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
        "build",
    )?;
    for step in &mut pipeline {
        for (key, value) in &recipe.build.environment {
            let value = template::expand_string(value, &scope)?;
            step.env.entry(key.clone()).or_insert(value);
        }
    }
    let ExpandedOutputs {
        outputs,
        tests: output_tests,
    } = expand_outputs(
        &recipe.outputs,
        &scope,
        options.step_root,
        options.allow_builtin_override,
    )?;
    let mut tests = expand_tests(
        &recipe.tests,
        &scope,
        options.step_root,
        options.allow_builtin_override,
        None,
        "tests",
    )?;
    tests.extend(output_tests);

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

#[cfg(test)]
mod tests {
    use super::{plan, step_labels};
    use crate::model::Step;

    const TEST_RECIPE: &str = r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: 1
  summary: probe
  license: MIT
build:
  pipeline:
    - runs: "true"
outputs:
  - name: probe
    metapackage: true
tests:
  - name: installed
    stage: installed
    environment:
      contents:
        packages: [python3, "${package.name}-test"]
      environment:
        HOME: /tmp
    pipeline:
      - runs: "true"
"#;

    const OUTPUT_TEST_RECIPE: &str = r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: 1
  summary: probe
  license: MIT
build:
  pipeline:
    - runs: "true"
outputs:
  - name: one
    metapackage: true
    tests:
      - name: smoke
        stage: installed
        argv: ["true"]
  - name: two
    metapackage: true
    tests:
      - name: smoke
        stage: installed
        argv: ["true"]
tests:
  - name: integration
    stage: installed
    argv: ["true"]
"#;

    fn runs(text: &str) -> Step {
        Step {
            runs: Some(text.to_string()),
            ..Step::default()
        }
    }

    #[test]
    fn the_first_output_may_use_the_main_package_fallback() {
        let recipe = crate::parse_recipe(
            r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: 1
  summary: probe
  license: MIT
outputs:
  - name: probe
"#,
        )
        .expect("recipe");
        let plan = plan(&recipe, "x86_64").expect("main output needs no selectors");
        assert!(plan.cells[0].outputs[0].files.is_empty());
    }

    #[test]
    fn file_presets_expand_to_globs() {
        let recipe = crate::parse_recipe(
            r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: 1
  summary: probe
  license: MIT
outputs:
  - name: probe
    metapackage: true
  - name: probe-dev
    files:
      include: ["@headers", "/usr/lib64/libprobe.so"]
"#,
        )
        .expect("recipe");
        let plan = plan(&recipe, "x86_64").expect("plan");
        assert_eq!(
            plan.cells[0].outputs[1].files,
            ["/usr/include/**", "/usr/lib64/libprobe.so"]
        );
    }

    #[test]
    fn an_unknown_file_preset_is_actionable() {
        let recipe = crate::parse_recipe(
            r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: 1
  summary: probe
  license: MIT
outputs:
  - name: probe
    files:
      include: ["@nope"]
"#,
        )
        .expect("recipe");
        let error = plan(&recipe, "x86_64").expect_err("unknown preset");
        assert_eq!(error.code, "RPMX-OUTPUT-019");
        assert!(error.message.contains("@nope"), "{}", error.message);
    }

    #[test]
    fn anonymous_steps_are_numbered_not_named_after_their_script() {
        let labels = step_labels(
            &[
                runs("cat > hello.c <<'SOURCE'"),
                runs("cat > hello.c <<'SOURCE'"),
            ],
            "build",
        );
        assert_eq!(labels, ["build:runs#1", "build:runs#2"]);
    }

    #[test]
    fn repeated_step_names_and_uses_get_distinct_suffixes() {
        let make = Step {
            uses: Some("make".to_string()),
            ..Step::default()
        };
        let named = Step {
            name: Some("make".to_string()),
            runs: Some("echo hi".to_string()),
            ..Step::default()
        };
        assert_eq!(
            step_labels(&[make.clone(), make, named], "build"),
            ["make", "make#2", "make#3"]
        );
    }

    #[test]
    fn test_environments_expand_packages_and_variables() {
        let recipe = crate::parse_recipe(TEST_RECIPE).expect("recipe");
        let plan = plan(&recipe, "x86_64").expect("plan");
        let test = &plan.cells[0].tests[0];
        assert_eq!(test.packages, ["python3", "probe-test"]);
        assert_eq!(
            test.environment.get("HOME").map(String::as_str),
            Some("/tmp")
        );
    }

    #[test]
    fn test_packages_are_rejected_on_build_stage() {
        let recipe = crate::parse_recipe(&TEST_RECIPE.replace("stage: installed", "stage: build"))
            .expect("recipe");
        let error = plan(&recipe, "x86_64").expect_err("build-stage packages");
        assert_eq!(error.code, rpmx_base::error::codes::TEST_004);
        assert!(
            error.message.contains("installed-stage"),
            "{}",
            error.message
        );
    }

    #[test]
    fn output_tests_keep_their_owner_and_can_repeat_names() {
        let recipe = crate::parse_recipe(OUTPUT_TEST_RECIPE).expect("recipe");
        let plan = plan(&recipe, "x86_64").expect("plan");
        let tests = &plan.cells[0].tests;
        assert_eq!(tests.len(), 3);
        assert_eq!(tests[0].qualified_name(), "integration");
        assert_eq!(tests[1].qualified_name(), "one/smoke");
        assert_eq!(tests[2].qualified_name(), "two/smoke");
        assert!(tests[1].matches_selector("one/smoke"));
        assert!(tests[1].matches_selector("smoke"));
        assert!(!tests[1].matches_selector("two/smoke"));
    }

    #[test]
    fn a_test_name_cannot_use_the_output_separator() {
        let recipe = crate::parse_recipe(&TEST_RECIPE.replace("name: installed", "name: bad/name"))
            .expect("recipe");
        let error = plan(&recipe, "x86_64").expect_err("separator");
        assert_eq!(error.code, rpmx_base::error::codes::TEST_001);
        assert!(
            error.message.contains("cannot contain '/'"),
            "{}",
            error.message
        );
    }

    #[test]
    fn explicit_names_win_over_uses_and_run_text() {
        let step = Step {
            name: Some("compile".to_string()),
            uses: Some("make".to_string()),
            runs: Some("echo hi".to_string()),
            ..Step::default()
        };
        assert_eq!(step_labels(&[step], "build"), ["compile"]);
    }
}
