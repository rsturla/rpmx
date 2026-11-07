//! Bootstrap manifests: ordered stages that rebuild a toolchain and a
//! distribution from a pinned seed.
//!
//! This module is intentionally pure. It parses and validates the manifest,
//! discovers recipes, computes one stage graph per architecture, and produces
//! a reviewable plan. Resolving roots and building stages belong to the CLI.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};
use rpmx_base::yaml;

use crate::bundle::{load_recipe_bundle, RecipeBundle};
use crate::model::Recipe;

pub const DOCUMENT: &str = "rpmx-bootstrap";
pub const SCHEMA_VERSION: u32 = 1;

fn default_document() -> String {
    DOCUMENT.to_string()
}

fn default_version() -> u32 {
    SCHEMA_VERSION
}

fn default_recipe_root() -> String {
    ".".to_string()
}

fn default_roots() -> String {
    "bootstrap/roots".to_string()
}

fn default_locks() -> String {
    "bootstrap/locks".to_string()
}

fn default_max_iterations() -> u32 {
    3
}

/// A scalar or list of strings. `packages: all` is the important scalar form;
/// accepting it everywhere keeps the manifest vocabulary small.
fn string_or_list<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_yaml_ng::Value::deserialize(deserializer)?;
    match value {
        serde_yaml_ng::Value::Null => Ok(Vec::new()),
        serde_yaml_ng::Value::String(text) => Ok(vec![text]),
        serde_yaml_ng::Value::Sequence(values) => values
            .into_iter()
            .map(|value| match value {
                serde_yaml_ng::Value::String(text) => Ok(text),
                other => Err(serde::de::Error::custom(format!(
                    "expected a string, got {other:?}"
                ))),
            })
            .collect(),
        other => Err(serde::de::Error::custom(format!(
            "expected a string or list of strings, got {other:?}"
        ))),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapManifest {
    #[serde(default = "default_document")]
    pub document: String,
    #[serde(default = "default_version", alias = "schema")]
    pub version: u32,
    pub name: String,
    #[serde(default, deserialize_with = "string_or_list")]
    pub archs: Vec<String>,
    #[serde(default = "default_recipe_root", rename = "recipe-root")]
    pub recipe_root: String,
    #[serde(default = "default_roots")]
    pub roots: String,
    #[serde(default = "default_locks")]
    pub locks: String,
    #[serde(default, rename = "source-date-epoch", alias = "source_date_epoch")]
    pub source_date_epoch: Option<u32>,
    #[serde(default)]
    pub seed: Option<Seed>,
    /// Maximum fixed-point passes per stage. The first pass is the ordinary
    /// stage build; each following pass rebuilds only `fixed-point` recipes.
    #[serde(
        default = "default_max_iterations",
        rename = "max-iterations",
        alias = "max_iterations"
    )]
    pub max_iterations: u32,
    pub stages: Vec<Stage>,
}

/// Provenance for the seed the ladder starts from. It is metadata, not a new
/// resolution mechanism: the stage profile and its root lock remain the source
/// of truth.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Seed {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub releasever: Option<String>,
    #[serde(default)]
    pub generation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub name: String,
    pub profile: String,
    #[serde(default, deserialize_with = "string_or_list")]
    pub packages: Vec<String>,
    #[serde(default, deserialize_with = "string_or_list")]
    pub recipes: Vec<String>,
    pub output: String,
    #[serde(default, deserialize_with = "string_or_list")]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub seed: bool,
    #[serde(default, rename = "extra-tools", deserialize_with = "string_or_list")]
    pub extra_tools: Vec<String>,
    #[serde(default, rename = "extra-sysroot", deserialize_with = "string_or_list")]
    pub extra_sysroot: Vec<String>,
    #[serde(default)]
    pub order: Vec<[String; 2]>,
    #[serde(default, rename = "fixed-point", deserialize_with = "string_or_list")]
    pub fixed_point: Vec<String>,
    #[serde(default, rename = "source-date-epoch", alias = "source_date_epoch")]
    pub source_date_epoch: Option<u32>,
}

/// A stage as it appears in a plan. Paths are relative to the manifest file.
#[derive(Debug, Clone, Serialize)]
pub struct PlannedStage {
    pub name: String,
    pub profile: String,
    pub arch: String,
    pub inputs: Vec<String>,
    pub output: String,
    pub root: String,
    pub lock: String,
    pub repo: String,
    pub repo_id: String,
    pub seed: bool,
    pub extra_tools: Vec<String>,
    pub extra_sysroot: Vec<String>,
    pub fixed_point: Vec<String>,
    pub packages: Vec<String>,
    pub recipes: Vec<String>,
    pub order: Vec<String>,
    pub source_date_epoch: Option<u32>,
}

impl PlannedStage {
    /// The selected package names and their recipe paths, in build order.
    ///
    /// The plan keeps the two arrays because they are useful in JSON, but
    /// callers that need both values should not zip them by hand.
    pub fn selected_recipes(&self) -> impl Iterator<Item = (&str, &str)> {
        debug_assert_eq!(
            self.packages.len(),
            self.recipes.len(),
            "the plan must keep package names and recipe paths aligned"
        );
        self.packages
            .iter()
            .map(String::as_str)
            .zip(self.recipes.iter().map(String::as_str))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapPlan {
    pub document: &'static str,
    pub version: u32,
    pub manifest: String,
    pub name: String,
    pub archs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<Seed>,
    pub source_date_epoch: Option<u32>,
    pub max_iterations: u32,
    pub stages: Vec<PlannedStage>,
}

/// Parse a manifest with the project-wide strict YAML policy.
pub fn parse_manifest(text: &str) -> Result<BootstrapManifest> {
    let manifest: BootstrapManifest = yaml::parse_strict(text, codes::BOOTSTRAP_001)?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

/// Load and validate a manifest from disk.
pub fn load_manifest(path: &Path) -> Result<BootstrapManifest> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        manifest_error(
            codes::BOOTSTRAP_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    parse_manifest(&text)
}

/// Load a manifest and plan every requested architecture.
pub fn plan_manifest(path: &Path, arch: Option<&str>) -> Result<BootstrapPlan> {
    let manifest = load_manifest(path)?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    plan_with_base(&manifest, base, path, arch)
}

/// Plan a manifest that has already been parsed. `base` is the directory the
/// manifest's relative paths are resolved against.
pub fn plan_with_base(
    manifest: &BootstrapManifest,
    base: &Path,
    manifest_path: &Path,
    arch: Option<&str>,
) -> Result<BootstrapPlan> {
    validate_manifest(manifest)?;
    let base = absolute_base(base);
    let archs = select_arches(manifest, arch)?;
    let metas = build_stage_metas(manifest, &base)?;
    validate_seed(manifest, &metas)?;

    let top_epoch = manifest
        .source_date_epoch
        .or_else(|| metas.first().and_then(|stage| stage.source_date_epoch));

    let mut stages = Vec::with_capacity(metas.len() * archs.len());
    for arch in &archs {
        for meta in &metas {
            // Validate that every selected recipe plans to exactly one cell for
            // this architecture. The plan itself is not stored: the stage
            // builder will plan again with its final policy.
            for selected in &meta.selected {
                let plan = crate::plan::plan(&selected.recipe, arch)?;
                if plan.cells.len() != 1 {
                    return Err(manifest_error(
                        codes::BOOTSTRAP_003,
                        format!(
                            "stage '{}': recipe {} expands to {} cells; bootstrap v1 requires \
                             one cell per recipe",
                            meta.name,
                            selected.display,
                            plan.cells.len()
                        ),
                    )
                    .at(format!("stages.{}.recipes", meta.name)));
                }
            }
            stages.push(meta.planned(arch));
        }
    }

    Ok(BootstrapPlan {
        document: "rpmx-bootstrap-plan",
        version: SCHEMA_VERSION,
        manifest: manifest_path.display().to_string(),
        name: manifest.name.clone(),
        archs,
        seed: manifest.seed.clone(),
        source_date_epoch: top_epoch,
        max_iterations: manifest.max_iterations,
        stages,
    })
}

fn validate_manifest(manifest: &BootstrapManifest) -> Result<()> {
    if manifest.document != DOCUMENT {
        return Err(manifest_error(
            codes::BOOTSTRAP_001,
            format!(
                "unsupported bootstrap document '{}'; expected '{DOCUMENT}'",
                manifest.document
            ),
        ));
    }
    if manifest.version != SCHEMA_VERSION {
        return Err(manifest_error(
            codes::BOOTSTRAP_001,
            format!(
                "unsupported bootstrap manifest version {}; this rpmx supports version {SCHEMA_VERSION}",
                manifest.version
            ),
        ));
    }
    if manifest.name.trim().is_empty() {
        return Err(manifest_error(
            codes::BOOTSTRAP_001,
            "bootstrap manifest has no name",
        ));
    }
    if !is_safe_name(&manifest.name) {
        return Err(manifest_error(
            codes::BOOTSTRAP_001,
            format!(
                "bootstrap name '{}' is not a safe identifier; use letters, digits, '.', '_' or '-'",
                manifest.name
            ),
        ));
    }
    if manifest.stages.is_empty() {
        return Err(manifest_error(
            codes::BOOTSTRAP_001,
            "bootstrap manifest declares no stages",
        ));
    }
    if manifest.max_iterations == 0 || manifest.max_iterations > 100 {
        return Err(manifest_error(
            codes::BOOTSTRAP_001,
            format!(
                "max-iterations must be between 1 and 100, got {}",
                manifest.max_iterations
            ),
        ));
    }
    let mut names = BTreeSet::new();
    for (index, stage) in manifest.stages.iter().enumerate() {
        if !is_safe_name(&stage.name) {
            return Err(manifest_error(
                codes::BOOTSTRAP_002,
                format!(
                    "stage name '{}' is not a safe identifier; use letters, digits, '.', '_' or '-'",
                    stage.name
                ),
            )
            .at(format!("stages[{index}].name")));
        }
        if !names.insert(stage.name.as_str()) {
            return Err(manifest_error(
                codes::BOOTSTRAP_002,
                format!("duplicate stage name '{}'", stage.name),
            )
            .at(format!("stages[{index}].name")));
        }
    }
    Ok(())
}

fn select_arches(manifest: &BootstrapManifest, requested: Option<&str>) -> Result<Vec<String>> {
    let mut allowed = Vec::new();
    for arch in &manifest.archs {
        let normalized = Arch::parse(arch)
            .map_err(|error| {
                manifest_error(
                    codes::BOOTSTRAP_009,
                    format!("bootstrap manifest has an invalid architecture '{arch}': {error}"),
                )
            })?
            .as_str()
            .to_string();
        if !allowed.contains(&normalized) {
            allowed.push(normalized);
        }
    }

    if let Some(arch) = requested {
        let normalized = Arch::parse(arch)
            .map_err(|error| {
                manifest_error(
                    codes::BOOTSTRAP_009,
                    format!("invalid architecture '{arch}': {error}"),
                )
            })?
            .as_str()
            .to_string();
        if !allowed.is_empty() && !allowed.contains(&normalized) {
            return Err(manifest_error(
                codes::BOOTSTRAP_009,
                format!(
                    "architecture '{normalized}' is not in the manifest; available architectures: {}",
                    allowed.join(", ")
                ),
            ));
        }
        return Ok(vec![normalized]);
    }

    if !allowed.is_empty() {
        return Ok(allowed);
    }
    Ok(vec![rpmx_base::arch::native_arch().to_string()])
}

struct StageMeta {
    name: String,
    profile: String,
    profile_path: PathBuf,
    output: String,
    root: String,
    lock: String,
    inputs: Vec<String>,
    seed: bool,
    extra_tools: Vec<String>,
    extra_sysroot: Vec<String>,
    fixed_point: Vec<String>,
    selected: Vec<SelectedRecipe>,
    package_order: Vec<String>,
    source_date_epoch: Option<u32>,
}

impl StageMeta {
    fn planned(&self, arch: &str) -> PlannedStage {
        let output = Path::new(&self.output);
        let repo = output.join(arch);
        PlannedStage {
            name: self.name.clone(),
            profile: self.profile.clone(),
            arch: arch.to_string(),
            inputs: self.inputs.clone(),
            output: self.output.clone(),
            root: self.root.clone(),
            lock: self.lock.clone(),
            repo: repo.display().to_string(),
            repo_id: format!("bootstrap-{}", self.name),
            seed: self.seed,
            extra_tools: self.extra_tools.clone(),
            extra_sysroot: self.extra_sysroot.clone(),
            fixed_point: self.fixed_point.clone(),
            packages: self.selected.iter().map(|item| item.name.clone()).collect(),
            recipes: self
                .selected
                .iter()
                .map(|item| item.display.clone())
                .collect(),
            order: self.package_order.clone(),
            source_date_epoch: self.source_date_epoch,
        }
    }
}

struct SelectedRecipe {
    name: String,
    display: String,
    recipe: Recipe,
}

#[derive(Debug, Default, Deserialize)]
struct ProfileProbe {
    #[serde(default)]
    releasever: Option<String>,
    #[serde(default)]
    builder: BuilderProbe,
    #[serde(default)]
    cross: Option<serde_yaml_ng::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct BuilderProbe {
    #[serde(default)]
    image: Option<String>,
}

fn build_stage_metas(manifest: &BootstrapManifest, base: &Path) -> Result<Vec<StageMeta>> {
    let recipe_root = resolve(base, &manifest.recipe_root, "recipe-root")?;
    // Validate the root/lock paths, but keep the manifest's spelling in the
    // plan so output stays reviewable and does not leak the caller's cwd.
    resolve(base, &manifest.roots, "roots")?;
    resolve(base, &manifest.locks, "locks")?;
    let roots = Path::new(&manifest.roots);
    let locks = Path::new(&manifest.locks);

    let mut seen_names = BTreeSet::new();
    let mut seen_outputs = BTreeMap::new();
    let mut metas: Vec<StageMeta> = Vec::with_capacity(manifest.stages.len());
    let last = manifest.stages.len() - 1;

    for (index, stage) in manifest.stages.iter().enumerate() {
        if !is_safe_name(&stage.name) {
            return Err(manifest_error(
                codes::BOOTSTRAP_002,
                format!(
                    "stage name '{}' is not a safe identifier; use letters, digits, '.', '_' or '-'",
                    stage.name
                ),
            )
            .at(format!("stages[{index}].name")));
        }
        if !seen_names.insert(stage.name.clone()) {
            return Err(manifest_error(
                codes::BOOTSTRAP_002,
                format!("duplicate stage name '{}'", stage.name),
            )
            .at(format!("stages[{index}].name")));
        }

        let profile_path = resolve(base, &stage.profile, "profile")?;
        let output_path = resolve(base, &stage.output, "output")?;
        if let Some(previous) = seen_outputs.insert(output_path.clone(), stage.name.clone()) {
            return Err(manifest_error(
                codes::BOOTSTRAP_002,
                format!(
                    "stages '{}' and '{}' share the output directory {}",
                    previous,
                    stage.name,
                    output_path.display()
                ),
            )
            .at(format!("stages[{index}].output")));
        }

        let probe = probe_profile(&profile_path, &stage.name)?;
        let crossing = probe.cross.is_some();
        let has_all = stage.packages.iter().any(|package| package == "all");

        if has_all {
            if index != last {
                return Err(manifest_error(
                    codes::BOOTSTRAP_003,
                    format!(
                        "stage '{}': `packages: all` is allowed only for the final self-hosted \
                         stage '{}'",
                        stage.name, manifest.stages[last].name
                    ),
                )
                .at(format!("stages[{index}].packages")));
            }
            if stage.seed || crossing {
                return Err(manifest_error(
                    codes::BOOTSTRAP_003,
                    format!(
                        "stage '{}': {} stage cannot use `packages: all`; name explicit packages \
                         or recipes so a new recipe cannot enter the bootstrap closure",
                        stage.name,
                        if stage.seed { "a seed" } else { "a cross" }
                    ),
                )
                .at(format!("stages[{index}].packages")));
            }
        }
        if (stage.seed || crossing) && stage.packages.is_empty() && stage.recipes.is_empty() {
            return Err(manifest_error(
                codes::BOOTSTRAP_003,
                format!(
                    "stage '{}': {} stage must name explicit packages or recipes",
                    stage.name,
                    if stage.seed { "a seed" } else { "a cross" }
                ),
            )
            .at(format!("stages[{index}].packages")));
        }

        let inputs = if stage.inputs.is_empty() {
            if index == 0 {
                Vec::new()
            } else {
                vec![manifest.stages[index - 1].name.clone()]
            }
        } else {
            stage.inputs.clone()
        };
        let mut unique_inputs = BTreeSet::new();
        for input in &inputs {
            if !unique_inputs.insert(input.clone()) {
                return Err(manifest_error(
                    codes::BOOTSTRAP_002,
                    format!(
                        "stage '{}' lists input '{input}' more than once",
                        stage.name
                    ),
                )
                .at(format!("stages[{index}].inputs")));
            }
            if !seen_names.contains(input) || input == &stage.name {
                return Err(manifest_error(
                    codes::BOOTSTRAP_002,
                    format!(
                        "stage '{}' has input '{input}', which is not an earlier stage; inputs \
                         must name an already-declared stage",
                        stage.name
                    ),
                )
                .at(format!("stages[{index}].inputs")));
            }
        }

        let selected = select_recipes(stage, index, &recipe_root, base)?;
        let names: Vec<String> = selected.iter().map(|item| item.name.clone()).collect();
        validate_stage_names(stage, index, &names)?;

        let source_date_epoch = effective_epoch(manifest, stage, index, &selected)?;
        let package_order = package_order(&stage.name, &names, &stage.order, &selected, stage)?;

        metas.push(StageMeta {
            name: stage.name.clone(),
            profile: stage.profile.clone(),
            profile_path,
            output: stage.output.clone(),
            root: roots.join(&stage.name).display().to_string(),
            lock: locks
                .join(format!("{}.lock.yaml", stage.name))
                .display()
                .to_string(),
            inputs,
            seed: stage.seed,
            extra_tools: stage.extra_tools.clone(),
            extra_sysroot: stage.extra_sysroot.clone(),
            fixed_point: stage.fixed_point.clone(),
            selected,
            package_order,
            source_date_epoch,
        });
    }
    Ok(metas)
}

fn validate_stage_names(stage: &Stage, index: usize, names: &[String]) -> Result<()> {
    for name in &stage.fixed_point {
        if !names.contains(name) {
            return Err(manifest_error(
                codes::BOOTSTRAP_002,
                format!(
                    "stage '{}': fixed-point package '{name}' is not one of the stage's selected \
                     packages",
                    stage.name
                ),
            )
            .at(format!("stages[{index}].fixed-point")));
        }
    }
    for edge in &stage.order {
        for name in edge {
            if !names.contains(name) {
                return Err(manifest_error(
                    codes::BOOTSTRAP_002,
                    format!(
                        "stage '{}': order edge names '{name}', which is not one of the stage's \
                         selected packages",
                        stage.name
                    ),
                )
                .at(format!("stages[{index}].order")));
            }
        }
    }
    Ok(())
}

fn select_recipes(
    stage: &Stage,
    index: usize,
    recipe_root: &Path,
    base: &Path,
) -> Result<Vec<SelectedRecipe>> {
    let has_all = stage.packages.iter().any(|package| package == "all");
    let explicit_packages: BTreeSet<&str> = stage
        .packages
        .iter()
        .map(String::as_str)
        .filter(|package| *package != "all")
        .collect();

    let candidates = if stage.recipes.is_empty() {
        discover_recipes(recipe_root).map_err(|error| {
            manifest_error(
                codes::BOOTSTRAP_003,
                format!(
                    "stage '{}': cannot discover recipes under {}: {}",
                    stage.name,
                    recipe_root.display(),
                    error.message
                ),
            )
            .at(format!("stages[{index}].recipes"))
        })?
    } else {
        select_recipe_paths(stage, index, base, recipe_root)?
    };

    if candidates.is_empty() {
        return Err(manifest_error(
            codes::BOOTSTRAP_003,
            format!(
                "stage '{}' selects no recipes; add packages/recipes or check recipe-root {}",
                stage.name,
                recipe_root.display()
            ),
        )
        .at(format!("stages[{index}].recipes")));
    }

    let mut loaded = Vec::with_capacity(candidates.len());
    for path in candidates {
        let display = path.display().to_string();
        let bundle = load_recipe_bundle(&path).map_err(|error| {
            manifest_error(
                codes::BOOTSTRAP_003,
                format!(
                    "stage '{}': cannot load recipe {}: {}",
                    stage.name, display, error.message
                ),
            )
            .at(format!("stages[{index}].recipes"))
        })?;
        reject_multi_cell(stage, index, &display, &bundle)?;
        loaded.push((display, bundle));
    }

    let mut available = BTreeMap::new();
    for (display, bundle) in loaded {
        let name = bundle.recipe.package.name.clone();
        if let Some(previous) = available.insert(name.clone(), (display.clone(), bundle)) {
            return Err(manifest_error(
                codes::BOOTSTRAP_003,
                format!(
                    "stage '{}': package '{name}' is defined by both {} and {}",
                    stage.name, previous.0, display
                ),
            )
            .at(format!("stages[{index}].recipes")));
        }
    }

    let mut selected = Vec::new();
    if has_all {
        for (name, (display, bundle)) in &available {
            selected.push(SelectedRecipe {
                name: name.clone(),
                display: display.clone(),
                recipe: bundle.recipe.clone(),
            });
        }
        for name in &explicit_packages {
            if !available.contains_key(*name) {
                return Err(missing_packages(stage, index, &[*name], &available));
            }
        }
    } else if explicit_packages.is_empty() {
        for (name, (display, bundle)) in &available {
            selected.push(SelectedRecipe {
                name: name.clone(),
                display: display.clone(),
                recipe: bundle.recipe.clone(),
            });
        }
    } else {
        let missing: Vec<&str> = explicit_packages
            .iter()
            .copied()
            .filter(|name| !available.contains_key(*name))
            .collect();
        if !missing.is_empty() {
            return Err(missing_packages(stage, index, &missing, &available));
        }
        for name in &explicit_packages {
            let (display, bundle) = available.get(*name).expect("checked above");
            selected.push(SelectedRecipe {
                name: (*name).to_string(),
                display: display.clone(),
                recipe: bundle.recipe.clone(),
            });
        }
    }

    if selected.is_empty() {
        return Err(manifest_error(
            codes::BOOTSTRAP_003,
            format!("stage '{}' selects no recipes", stage.name),
        )
        .at(format!("stages[{index}].recipes")));
    }
    Ok(selected)
}

fn missing_packages(
    stage: &Stage,
    index: usize,
    missing: &[&str],
    available: &BTreeMap<String, (String, RecipeBundle)>,
) -> Error {
    let names: Vec<&str> = available.keys().map(String::as_str).collect();
    manifest_error(
        codes::BOOTSTRAP_003,
        format!(
            "stage '{}': package(s) {} not found; available packages: {}",
            stage.name,
            missing
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>()
                .join(", "),
            if names.is_empty() {
                "<none>".to_string()
            } else {
                names.join(", ")
            }
        ),
    )
    .at(format!("stages[{index}].packages"))
}

fn reject_multi_cell(
    stage: &Stage,
    index: usize,
    display: &str,
    bundle: &RecipeBundle,
) -> Result<()> {
    let cells = crate::matrix::expand(bundle.recipe.matrix.as_ref()).map_err(|error| {
        manifest_error(
            codes::BOOTSTRAP_003,
            format!(
                "stage '{}': recipe {display} has an invalid matrix: {}",
                stage.name, error.message
            ),
        )
        .at(format!("stages[{index}].recipes"))
    })?;
    if cells.len() > 1 {
        return Err(manifest_error(
            codes::BOOTSTRAP_003,
            format!(
                "stage '{}': recipe {display} expands to {} cells; bootstrap v1 requires one \
                 cell per recipe so package names stay unambiguous",
                stage.name,
                cells.len()
            ),
        )
        .at(format!("stages[{index}].recipes")));
    }
    Ok(())
}

fn select_recipe_paths(
    stage: &Stage,
    index: usize,
    base: &Path,
    recipe_root: &Path,
) -> Result<Vec<PathBuf>> {
    let mut selected = BTreeSet::new();
    for raw in &stage.recipes {
        let path = resolve(base, raw, "recipes")?;
        let matches = recipe_matches(&path, recipe_root).map_err(|error| {
            manifest_error(
                codes::BOOTSTRAP_003,
                format!(
                    "stage '{}': cannot expand recipe pattern '{}': {}",
                    stage.name, raw, error.message
                ),
            )
            .at(format!("stages[{index}].recipes"))
        })?;
        if matches.is_empty() {
            return Err(manifest_error(
                codes::BOOTSTRAP_003,
                format!(
                    "stage '{}': recipe pattern '{}' matches no package.yaml",
                    stage.name, raw
                ),
            )
            .at(format!("stages[{index}].recipes")));
        }
        selected.extend(matches);
    }
    Ok(selected.into_iter().collect())
}

fn recipe_matches(pattern: &Path, recipe_root: &Path) -> Result<Vec<PathBuf>> {
    let text = pattern.to_string_lossy();
    let has_glob = text.contains(['*', '?', '[']);
    if !has_glob {
        if pattern.is_file() {
            return Ok(vec![canonical(pattern)?]);
        }
        if pattern.is_dir() {
            let package = pattern.join("package.yaml");
            if package.is_file() {
                return Ok(vec![canonical(&package)?]);
            }
        }
        return Ok(Vec::new());
    }

    let root = glob_root(pattern);
    let mut candidates = Vec::new();
    if root.is_dir() {
        walk_package_files(&root, &mut candidates)?;
    }
    if recipe_root.is_dir() {
        walk_package_files(recipe_root, &mut candidates)?;
    }
    candidates.sort();
    candidates.dedup();

    let glob = globset::GlobBuilder::new(&pattern.to_string_lossy())
        .literal_separator(false)
        .build()
        .map_err(|error| {
            manifest_error(
                codes::BOOTSTRAP_003,
                format!("invalid recipe pattern '{}': {error}", pattern.display()),
            )
        })?;
    let matcher = glob.compile_matcher();
    let mut matches = Vec::new();
    for candidate in candidates {
        if matcher.is_match(&candidate) {
            matches.push(canonical(&candidate)?);
        }
    }
    matches.sort();
    matches.dedup();
    Ok(matches)
}

fn glob_root(pattern: &Path) -> PathBuf {
    let mut root = PathBuf::new();
    for component in pattern.components() {
        let text = component.as_os_str().to_string_lossy();
        if text.contains(['*', '?', '[']) {
            break;
        }
        root.push(component.as_os_str());
    }
    if root.as_os_str().is_empty() {
        a_parent(pattern)
    } else if root.extension().is_some() && !root.is_dir() {
        a_parent(&root)
    } else {
        root
    }
}

fn a_parent(path: &Path) -> PathBuf {
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn discover_recipes(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Err(manifest_error(
            codes::BOOTSTRAP_003,
            format!("recipe root {} is not a directory", root.display()),
        ));
    }
    let mut recipes = Vec::new();
    walk_package_files(root, &mut recipes)?;
    recipes.sort();
    recipes.dedup();
    Ok(recipes)
}

fn walk_package_files(directory: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        manifest_error(
            codes::BOOTSTRAP_003,
            format!("cannot read {}: {error}", directory.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            manifest_error(
                codes::BOOTSTRAP_003,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            manifest_error(
                codes::BOOTSTRAP_003,
                format!("cannot inspect {}: {error}", path.display()),
            )
        })?;
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules") {
                continue;
            }
            walk_package_files(&path, out)?;
        } else if file_type.is_file() && entry.file_name() == "package.yaml" {
            out.push(canonical(&path)?);
        }
    }
    Ok(())
}

fn canonical(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).map_err(|error| {
        manifest_error(
            codes::BOOTSTRAP_003,
            format!("cannot resolve {}: {error}", path.display()),
        )
    })
}

fn effective_epoch(
    manifest: &BootstrapManifest,
    stage: &Stage,
    index: usize,
    selected: &[SelectedRecipe],
) -> Result<Option<u32>> {
    if let Some(epoch) = stage.source_date_epoch {
        return Ok(Some(epoch));
    }
    if let Some(epoch) = manifest.source_date_epoch {
        return Ok(Some(epoch));
    }
    let mut values: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for item in selected {
        if let Some(epoch) = item.recipe.package.source_date_epoch {
            values.entry(epoch).or_default().push(item.display.clone());
        }
    }
    match values.len() {
        0 => Ok(None),
        1 => Ok(values.keys().next().copied()),
        _ => {
            let detail = values
                .iter()
                .map(|(epoch, recipes)| format!("{epoch} ({})", recipes.join(", ")))
                .collect::<Vec<_>>()
                .join("; ");
            Err(manifest_error(
                codes::BOOTSTRAP_001,
                format!(
                    "stage '{}': recipes disagree about source-date-epoch: {detail}; set \
                     source-date-epoch on the stage or manifest",
                    stage.name
                ),
            )
            .at(format!("stages[{index}].source-date-epoch")))
        }
    }
}

fn package_order(
    stage_name: &str,
    names: &[String],
    declared: &[[String; 2]],
    selected: &[SelectedRecipe],
    stage: &Stage,
) -> Result<Vec<String>> {
    let by_name: BTreeMap<&str, &SelectedRecipe> = selected
        .iter()
        .map(|item| (item.name.as_str(), item))
        .collect();
    let mut edges: BTreeMap<String, BTreeSet<String>> = names
        .iter()
        .map(|name| (name.clone(), BTreeSet::new()))
        .collect();

    for item in selected {
        let deps = &item.recipe.build.dependencies;
        for dependency in deps
            .tools
            .iter()
            .chain(deps.host.iter())
            .chain(deps.target.iter())
            .chain(deps.tests.iter())
        {
            if dependency == &item.name || !by_name.contains_key(dependency.as_str()) {
                continue;
            }
            edges
                .entry(dependency.clone())
                .or_default()
                .insert(item.name.clone());
        }
    }

    for edge in declared {
        if let Some(targets) = edges.get_mut(&edge[0]) {
            targets.insert(edge[1].clone());
        }
    }

    let fixed: BTreeSet<String> = stage.fixed_point.iter().cloned().collect();
    topological_order(names, edges, &fixed).map_err(|cycle| {
        manifest_error(
            codes::BOOTSTRAP_004,
            format!(
                "stage '{stage_name}': dependency cycle among {}; add an `order` edge or list \
                 every cycle member in `fixed-point`",
                cycle.join(" -> ")
            ),
        )
    })
}

fn topological_order(
    names: &[String],
    mut edges: BTreeMap<String, BTreeSet<String>>,
    fixed: &BTreeSet<String>,
) -> std::result::Result<Vec<String>, Vec<String>> {
    let positions: BTreeMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    let limit = names
        .len()
        .saturating_mul(names.len())
        .saturating_add(names.len());
    for _ in 0..=limit {
        if let Some(order) = kahn(names, &edges, &positions) {
            return Ok(order);
        }
        let Some(cycle) = find_cycle(names, &edges) else {
            return Err(Vec::new());
        };
        if !cycle.iter().all(|name| fixed.contains(name)) {
            return Err(cycle);
        }
        let closing = cycle.last().expect("a cycle is not empty").clone();
        let first = cycle.first().expect("a cycle is not empty").clone();
        if let Some(targets) = edges.get_mut(&closing) {
            targets.remove(&first);
        }
    }
    Err(Vec::new())
}

fn kahn(
    names: &[String],
    edges: &BTreeMap<String, BTreeSet<String>>,
    positions: &BTreeMap<&str, usize>,
) -> Option<Vec<String>> {
    let mut indegree: BTreeMap<&str, usize> = names.iter().map(|name| (name.as_str(), 0)).collect();
    for targets in edges.values() {
        for target in targets {
            *indegree.get_mut(target.as_str()).expect("known target") += 1;
        }
    }
    let mut ready: BTreeSet<(usize, &str)> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(name, _)| (positions[name], *name))
        .collect();
    let mut order = Vec::with_capacity(names.len());
    while let Some((_, name)) = ready.pop_first() {
        order.push(name.to_string());
        if let Some(targets) = edges.get(name) {
            for target in targets {
                let degree = indegree.get_mut(target.as_str()).expect("known target");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert((positions[target.as_str()], target.as_str()));
                }
            }
        }
    }
    (order.len() == names.len()).then_some(order)
}

fn find_cycle(names: &[String], edges: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        White,
        Gray,
        Black,
    }
    fn visit(
        node: &str,
        edges: &BTreeMap<String, BTreeSet<String>>,
        state: &mut BTreeMap<String, State>,
        stack: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        state.insert(node.to_string(), State::Gray);
        stack.push(node.to_string());
        if let Some(targets) = edges.get(node) {
            for target in targets {
                match state.get(target).copied().unwrap_or(State::White) {
                    State::White => {
                        if let Some(cycle) = visit(target, edges, state, stack) {
                            return Some(cycle);
                        }
                    }
                    State::Gray => {
                        let position = stack
                            .iter()
                            .position(|item| item == target)
                            .expect("gray node is on the stack");
                        return Some(stack[position..].to_vec());
                    }
                    State::Black => {}
                }
            }
        }
        stack.pop();
        state.insert(node.to_string(), State::Black);
        None
    }

    let mut state: BTreeMap<String, State> = names
        .iter()
        .map(|name| (name.clone(), State::White))
        .collect();
    for name in names {
        if state.get(name).copied().unwrap_or(State::White) == State::White {
            let mut stack = Vec::new();
            if let Some(cycle) = visit(name, edges, &mut state, &mut stack) {
                return Some(cycle);
            }
        }
    }
    None
}

fn probe_profile(path: &Path, stage: &str) -> Result<ProfileProbe> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        manifest_error(
            codes::BOOTSTRAP_002,
            format!(
                "stage '{stage}': cannot read profile {}: {error}",
                path.display()
            ),
        )
    })?;
    yaml::parse_strict(&text, codes::BOOTSTRAP_001).map_err(|error| {
        manifest_error(
            codes::BOOTSTRAP_002,
            format!(
                "stage '{stage}': cannot parse profile {}: {}",
                path.display(),
                error.message
            ),
        )
    })
}

fn validate_seed(manifest: &BootstrapManifest, metas: &[StageMeta]) -> Result<()> {
    let Some(seed) = &manifest.seed else {
        return Ok(());
    };
    for (field, value) in [
        ("name", &seed.name),
        ("image", &seed.image),
        ("releasever", &seed.releasever),
        ("generation", &seed.generation),
    ] {
        if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
            return Err(manifest_error(
                codes::BOOTSTRAP_001,
                format!("seed.{field} is empty"),
            ));
        }
    }
    let Some(stage) = metas.first() else {
        return Ok(());
    };
    let probe = probe_profile(&stage.profile_path, &stage.name)?;
    if let (Some(declared), Some(profile)) = (&seed.releasever, &probe.releasever) {
        if declared != profile {
            return Err(manifest_error(
                codes::BOOTSTRAP_001,
                format!(
                    "seed.releasever is '{}' but stage '{}' profile {} declares releasever \
                     '{}'; the profile is the source of truth",
                    declared, stage.name, stage.profile, profile
                ),
            ));
        }
    }
    if let (Some(declared), Some(profile)) = (&seed.image, &probe.builder.image) {
        if declared != profile {
            return Err(manifest_error(
                codes::BOOTSTRAP_001,
                format!(
                    "seed.image is '{}' but stage '{}' profile {} declares builder.image '{}'; \
                     the profile is the source of truth",
                    declared, stage.name, stage.profile, profile
                ),
            ));
        }
    }
    Ok(())
}

fn absolute_base(base: &Path) -> PathBuf {
    if base.as_os_str().is_empty() {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf())
}

fn resolve(base: &Path, raw: &str, field: &str) -> Result<PathBuf> {
    if raw.trim().is_empty() {
        return Err(manifest_error(
            codes::BOOTSTRAP_002,
            format!("{field} path is empty"),
        ));
    }
    let path = Path::new(raw);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(manifest_error(
            codes::BOOTSTRAP_002,
            format!("{field} path '{raw}' must not contain '..'"),
        ));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(base.join(path))
    }
}

fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn manifest_error(code: rpmx_base::error::Code, message: impl Into<String>) -> Error {
    Error::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::{parse_manifest, plan_with_base};
    use rpmx_base::error::codes;
    use std::path::Path;

    fn manifest(extra: &str) -> String {
        format!(
            r#"
document: rpmx-bootstrap
version: 1
name: test
archs: [x86_64]
recipe-root: recipes
source-date-epoch: 1704067200
stages:
{extra}
"#
        )
    }

    fn write_recipe(root: &Path, name: &str, deps: &str) {
        let directory = root.join("recipes").join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("package.yaml"),
            format!(
                r#"
schema: 1
package:
  name: {name}
  version: "1"
  release: 1
  summary: {name}
  license: MIT
build:
  dependencies:
{deps}
outputs:
  - name: {name}
    metapackage: true
"#
            ),
        )
        .unwrap();
    }

    fn write_profile(root: &Path, name: &str, cross: bool) {
        let cross = if cross {
            "\ncross:\n  bootstrap: true\n  packages: [gcc]\n"
        } else {
            ""
        };
        std::fs::write(
            root.join(format!("{name}.yaml")),
            format!(
                r#"
document: rpmx-profile
version: 1
name: {name}
releasever: "44"
archs: [x86_64]
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
{cross}"#
            ),
        )
        .unwrap();
    }

    fn setup(extra: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write_recipe(root, "gcc", "    tools: []");
        write_recipe(root, "glibc", "    tools: [gcc]");
        write_profile(root, "stage0", true);
        write_profile(root, "stage1", false);
        let manifest_path = root.join("bootstrap.yaml");
        std::fs::write(&manifest_path, manifest(extra)).unwrap();
        (directory, manifest_path)
    }

    fn setup_for(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write_recipe(root, "gcc", "    tools: []");
        write_recipe(root, "glibc", "    tools: [gcc]");
        write_profile(root, "stage0", true);
        write_profile(root, "stage1", false);
        let manifest_path = root.join("bootstrap.yaml");
        std::fs::write(&manifest_path, text).unwrap();
        (directory, manifest_path)
    }

    fn setup_with_seed(extra: &str, releasever: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let (directory, manifest_path) = setup(extra);
        let text = std::fs::read_to_string(&manifest_path).unwrap().replace(
            "source-date-epoch: 1704067200",
            &format!(
                "source-date-epoch: 1704067200\nseed:\n  name: test-seed\n  releasever: \"{releasever}\""
            ),
        );
        std::fs::write(&manifest_path, text).unwrap();
        (directory, manifest_path)
    }

    #[test]
    fn a_plan_orders_stages_and_packages() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [glibc, gcc]
    output: out/stage0
    seed: true
  - name: stage1
    profile: stage1.yaml
    packages: all
    output: out
"#,
        );
        let plan = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            Some("x86_64"),
        )
        .unwrap();
        assert_eq!(plan.archs, vec!["x86_64"]);
        assert_eq!(plan.stages.len(), 2);
        assert_eq!(plan.stages[0].order, vec!["gcc", "glibc"]);
        assert!(plan.stages[0].seed);
        assert_eq!(plan.stages[0].repo, "out/stage0/x86_64");
        assert_eq!(plan.stages[1].inputs, vec!["stage0"]);
        assert_eq!(plan.source_date_epoch, Some(1704067200));
    }

    #[test]
    fn schema_is_an_alias_for_version() {
        let text = r#"
schema: 1
name: test
recipe-root: recipes
stages:
  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
"#;
        let manifest = parse_manifest(text).unwrap();
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.document, super::DOCUMENT);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let text = manifest(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
    surprise: true
"#,
        );
        let error = parse_manifest(&text).expect_err("unknown field");
        assert_eq!(error.code, codes::BOOTSTRAP_001);
    }

    #[test]
    fn seed_and_cross_stages_require_explicit_packages() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: all
    output: out
    seed: true
"#,
        );
        let error = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .expect_err("seed stage cannot use all");
        assert_eq!(error.code, codes::BOOTSTRAP_003);
    }

    #[test]
    fn all_is_only_allowed_for_the_final_stage() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: all
    output: out/stage0
  - name: stage1
    profile: stage1.yaml
    packages: [gcc]
    output: out
"#,
        );
        let error = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .expect_err("all before the final stage");
        assert_eq!(error.code, codes::BOOTSTRAP_003);
    }

    #[test]
    fn a_fixed_point_cycle_is_allowed_but_a_plain_cycle_is_not() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc, glibc]
    output: out
    order: [[glibc, gcc], [gcc, glibc]]
    fixed-point: [gcc, glibc]
"#,
        );
        let plan = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .unwrap();
        assert_eq!(plan.stages[0].order.len(), 2);

        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc, glibc]
    output: out
    order: [[glibc, gcc], [gcc, glibc]]
"#,
        );
        let error = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .expect_err("plain cycle");
        assert_eq!(error.code, codes::BOOTSTRAP_004);
        assert!(error.message.contains("gcc"), "{}", error.message);
    }

    #[test]
    fn recipe_epochs_must_agree_without_an_explicit_value() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write_recipe(root, "gcc", "    tools: []");
        write_recipe(root, "glibc", "    tools: []");
        write_profile(root, "stage0", false);
        let path = root.join("bootstrap.yaml");
        std::fs::write(
            &path,
            r#"
document: rpmx-bootstrap
version: 1
name: test
recipe-root: recipes
stages:
  - name: stage0
    profile: stage0.yaml
    packages: [gcc, glibc]
    output: out
"#,
        )
        .unwrap();
        // Rewrite one recipe with an epoch; the other has none, so the stage
        // derives the one explicit value. Two different values must fail.
        let recipe = root.join("recipes/glibc/package.yaml");
        let text = std::fs::read_to_string(&recipe).unwrap();
        std::fs::write(
            &recipe,
            text.replace("  license: MIT", "  license: MIT\n  source-date-epoch: 1"),
        )
        .unwrap();
        let recipe = root.join("recipes/gcc/package.yaml");
        let text = std::fs::read_to_string(&recipe).unwrap();
        std::fs::write(
            &recipe,
            text.replace("  license: MIT", "  license: MIT\n  source-date-epoch: 2"),
        )
        .unwrap();
        let error = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .expect_err("disagreeing epochs");
        assert_eq!(error.code, codes::BOOTSTRAP_001);
        assert!(error.message.contains("1"), "{}", error.message);
        assert!(error.message.contains("2"), "{}", error.message);
    }

    #[test]
    fn an_unknown_stage_input_is_rejected() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out/stage0
  - name: stage1
    profile: stage1.yaml
    packages: [gcc]
    output: out
    inputs: [stage2]
"#,
        );
        let error = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .expect_err("forward input");
        assert_eq!(error.code, codes::BOOTSTRAP_002);
    }

    #[test]
    fn an_unsupported_version_is_rejected() {
        let text = manifest(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
"#,
        )
        .replace("version: 1", "version: 2");
        let error = parse_manifest(&text).expect_err("unsupported version");
        assert_eq!(error.code, codes::BOOTSTRAP_001);
    }

    #[test]
    fn duplicate_stage_names_and_outputs_are_rejected() {
        let text = manifest(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: other
"#,
        );
        let error = parse_manifest(&text).expect_err("duplicate stage");
        assert_eq!(error.code, codes::BOOTSTRAP_002);

        let text = manifest(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
  - name: stage1
    profile: stage1.yaml
    packages: [gcc]
    output: out
"#,
        );
        let (_directory, path) = setup_for(&text);
        let error = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .expect_err("duplicate output");
        assert_eq!(error.code, codes::BOOTSTRAP_002);
    }

    #[test]
    fn duplicate_package_names_are_rejected() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
"#,
        );
        let root = path.parent().unwrap();
        std::fs::create_dir_all(root.join("recipes/other")).unwrap();
        std::fs::write(
            root.join("recipes/other/package.yaml"),
            r#"
schema: 1
package:
  name: gcc
  version: "1"
  release: 1
  summary: other gcc
  license: MIT
outputs:
  - name: gcc
    metapackage: true
"#,
        )
        .unwrap();
        let error = plan_with_base(&super::load_manifest(&path).unwrap(), root, &path, None)
            .expect_err("duplicate package");
        assert_eq!(error.code, codes::BOOTSTRAP_003);
        assert!(error.message.contains("recipes/other"), "{}", error.message);
    }

    #[test]
    fn a_missing_package_lists_the_available_names() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [missing]
    output: out
"#,
        );
        let error = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .expect_err("missing package");
        assert_eq!(error.code, codes::BOOTSTRAP_003);
        assert!(error.message.contains("gcc"), "{}", error.message);
        assert!(error.message.contains("glibc"), "{}", error.message);
    }

    #[test]
    fn seed_metadata_must_agree_with_the_first_profile() {
        let (_directory, path) = setup_with_seed(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
"#,
            "55",
        );
        let error = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .expect_err("seed mismatch");
        assert_eq!(error.code, codes::BOOTSTRAP_001);
        assert!(error.message.contains("55"), "{}", error.message);
        assert!(error.message.contains("44"), "{}", error.message);
    }

    #[test]
    fn source_date_epoch_precedence_is_stage_then_manifest() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
    source-date-epoch: 2
"#,
        );
        let plan = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .unwrap();
        assert_eq!(plan.stages[0].source_date_epoch, Some(2));

        let text = manifest(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
"#,
        );
        let (_directory, path) = setup_for(&text);
        let plan = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .unwrap();
        assert_eq!(plan.stages[0].source_date_epoch, Some(1704067200));
    }

    #[test]
    fn explicit_recipe_globs_filter_against_packages() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    recipes: [recipes/*/package.yaml]
    packages: [gcc]
    output: out
"#,
        );
        let plan = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .unwrap();
        assert_eq!(plan.stages[0].packages, vec!["gcc"]);
    }

    #[test]
    fn max_iterations_defaults_to_three_and_must_be_positive() {
        let text = manifest(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
"#,
        );
        let parsed = parse_manifest(&text).unwrap();
        assert_eq!(parsed.max_iterations, 3);
        let error = parse_manifest(&text.replace(
            "source-date-epoch: 1704067200",
            "source-date-epoch: 1704067200\nmax-iterations: 0",
        ))
        .expect_err("zero iterations");
        assert_eq!(error.code, codes::BOOTSTRAP_001);
    }

    #[test]
    fn a_multi_cell_recipe_is_rejected() {
        let (_directory, path) = setup(
            r#"  - name: stage0
    profile: stage0.yaml
    packages: [gcc]
    output: out
"#,
        );
        let recipe = path.parent().unwrap().join("recipes/gcc/package.yaml");
        let text = std::fs::read_to_string(&recipe).unwrap();
        std::fs::write(
            &recipe,
            format!("{text}\nmatrix:\n  axes:\n    flavor: [a, b]\n"),
        )
        .unwrap();
        let error = plan_with_base(
            &super::load_manifest(&path).unwrap(),
            path.parent().unwrap(),
            &path,
            None,
        )
        .expect_err("multi-cell");
        assert_eq!(error.code, codes::BOOTSTRAP_003);
    }
}
