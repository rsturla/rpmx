//! Compose one recipe from a root file and included fragments.
//!
//! Composition is deliberately structural and strict: every top-level section
//! has exactly one owner. There is no overlay merge, so a section cannot be
//! silently shadowed and every value has one file to point at.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rpmx_base::error::{codes, Error, Result};
use rpmx_base::hash::sha512_hex;

use crate::model::RecipeFragment;
use crate::{parse_recipe, Recipe};

/// One file in a recipe bundle.
#[derive(Debug, Clone)]
pub struct BundleFile {
    /// Canonical path on disk.
    pub path: PathBuf,
    /// Path relative to the root recipe's directory, using `/`.
    pub name: String,
    pub digest: String,
    pub bytes: Vec<u8>,
}

/// The merged recipe plus the files it came from.
#[derive(Debug, Clone)]
pub struct RecipeBundle {
    pub recipe: Recipe,
    /// Canonical path of the root recipe.
    pub root: PathBuf,
    /// Root first, then included fragments in discovery order.
    pub files: Vec<BundleFile>,
    digest: String,
    owners: BTreeMap<String, PathBuf>,
}

impl RecipeBundle {
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The file that owns a top-level section, such as `build`.
    pub fn owner(&self, section: &str) -> Option<&Path> {
        self.owners.get(section).map(PathBuf::as_path)
    }

    pub fn root_file(&self) -> &BundleFile {
        self.files.first().expect("a bundle always has a root file")
    }

    pub fn included_files(&self) -> impl Iterator<Item = &BundleFile> {
        self.files.iter().skip(1)
    }

    pub fn file(&self, path: &Path) -> Option<&BundleFile> {
        self.files.iter().find(|file| file.path == path)
    }
}

#[derive(Default)]
struct Merged {
    package: Option<crate::model::Package>,
    vars: Option<BTreeMap<String, serde_json::Value>>,
    build: Option<crate::model::Build>,
    sources: Option<Vec<crate::model::Source>>,
    patches: Option<Vec<crate::model::Patch>>,
    source_assembly: Option<crate::model::SourceAssembly>,
    matrix: Option<crate::model::Matrix>,
    outputs: Option<Vec<crate::model::Output>>,
    tests: Option<Vec<crate::model::Test>>,
}

/// Load a recipe, following `include:` entries relative to each file.
pub fn load_recipe_bundle(path: impl AsRef<Path>) -> Result<RecipeBundle> {
    let requested = path.as_ref();
    let root = std::fs::canonicalize(requested).map_err(|error| {
        Error::new(
            codes::INCLUDE_001,
            format!("cannot read {}: {error}", requested.display()),
        )
        .at(requested.display().to_string())
    })?;
    if !root.is_file() {
        return Err(Error::new(
            codes::INCLUDE_001,
            format!("{} is not a file", root.display()),
        )
        .at(root.display().to_string()));
    }
    let root_dir = root
        .parent()
        .ok_or_else(|| Error::new(codes::INCLUDE_001, "the recipe has no parent directory"))?
        .to_path_buf();

    let mut files = Vec::new();
    let mut owners = BTreeMap::new();
    let mut merged = Merged::default();
    let mut stack = Vec::new();
    let mut visited = BTreeSet::new();
    let schema = visit(
        &root,
        true,
        &root_dir,
        &mut stack,
        &mut visited,
        &mut files,
        &mut owners,
        &mut merged,
    )?;
    let Some(schema) = schema else {
        return Err(
            Error::new(codes::INCLUDE_003, "the root recipe must declare `schema`")
                .at(root.display().to_string()),
        );
    };
    let package = merged.package.ok_or_else(|| {
        Error::new(
            codes::INCLUDE_003,
            "the recipe bundle defines no `package` section",
        )
        .at(root.display().to_string())
    })?;

    let recipe = Recipe {
        schema,
        package,
        vars: merged.vars.unwrap_or_default(),
        build: merged.build.unwrap_or_default(),
        sources: merged.sources.unwrap_or_default(),
        patches: merged.patches.unwrap_or_default(),
        source_assembly: merged.source_assembly.unwrap_or_default(),
        matrix: merged.matrix,
        outputs: merged.outputs.unwrap_or_default(),
        tests: merged.tests.unwrap_or_default(),
    };
    // The merged document must satisfy the same strict schema as a single
    // file. This catches a fragment that is missing a required field.
    let canonical = serde_yaml_ng::to_string(&recipe)
        .map_err(|error| Error::new(codes::INCLUDE_003, error.to_string()))?;
    let _ = parse_recipe(&canonical)?;

    let mut entries: Vec<(String, String)> = files
        .iter()
        .map(|file| (file.name.clone(), file.digest.clone()))
        .collect();
    entries.sort();
    let encoded = serde_json::to_vec(&entries)
        .map_err(|error| Error::new(codes::INCLUDE_003, error.to_string()))?;
    let digest = sha512_hex(&encoded);

    Ok(RecipeBundle {
        recipe,
        root,
        files,
        digest,
        owners,
    })
}

#[allow(clippy::too_many_arguments)]
fn visit(
    path: &Path,
    is_root: bool,
    root_dir: &Path,
    stack: &mut Vec<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
    files: &mut Vec<BundleFile>,
    owners: &mut BTreeMap<String, PathBuf>,
    merged: &mut Merged,
) -> Result<Option<u32>> {
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        Error::new(
            codes::INCLUDE_001,
            format!("cannot read included recipe {}: {error}", path.display()),
        )
        .at(path.display().to_string())
    })?;
    if !canonical.starts_with(root_dir) {
        return Err(Error::new(
            codes::INCLUDE_004,
            format!(
                "included file {} escapes the recipe directory {}",
                canonical.display(),
                root_dir.display()
            ),
        )
        .at(path.display().to_string()));
    }
    if stack.contains(&canonical) {
        let mut chain: Vec<_> = stack.iter().map(|item| display(root_dir, item)).collect();
        chain.push(display(root_dir, &canonical));
        return Err(Error::new(
            codes::INCLUDE_005,
            format!("include cycle: {}", chain.join(" -> ")),
        ));
    }
    if !is_root && !visited.insert(canonical.clone()) {
        return Ok(None);
    }
    let bytes = std::fs::read(&canonical).map_err(|error| {
        Error::new(
            codes::INCLUDE_001,
            format!("cannot read {}: {error}", canonical.display()),
        )
        .at(canonical.display().to_string())
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        Error::new(
            codes::INCLUDE_001,
            format!("{} is not UTF-8: {error}", canonical.display()),
        )
        .at(canonical.display().to_string())
    })?;
    let fragment: RecipeFragment = rpmx_base::yaml::parse_strict(text, codes::YAML_001)?;
    if !is_root && fragment.schema.is_some() {
        return Err(Error::new(
            codes::INCLUDE_006,
            format!(
                "included fragment {} must not declare `schema`; only the root recipe does",
                canonical.display()
            ),
        )
        .at(canonical.display().to_string()));
    }
    let name = display(root_dir, &canonical);
    let digest = sha512_hex(&bytes);
    files.push(BundleFile {
        path: canonical.clone(),
        name,
        digest,
        bytes,
    });
    stack.push(canonical.clone());

    let mut schema = None;
    if is_root {
        schema = fragment.schema;
    }
    for include in &fragment.include {
        let include = include.trim();
        if include.is_empty() {
            return Err(Error::new(
                codes::INCLUDE_001,
                format!("{} contains an empty include path", canonical.display()),
            )
            .at(canonical.display().to_string()));
        }
        let parent = canonical.parent().unwrap_or(root_dir);
        let child = parent.join(include);
        let child_schema = visit(
            &child, false, root_dir, stack, visited, files, owners, merged,
        )?;
        debug_assert!(child_schema.is_none());
    }

    merge(fragment, &canonical, owners, merged)?;
    stack.pop();
    Ok(schema)
}

fn merge(
    fragment: RecipeFragment,
    path: &Path,
    owners: &mut BTreeMap<String, PathBuf>,
    merged: &mut Merged,
) -> Result<()> {
    macro_rules! set {
        ($field:ident, $key:literal, $value:expr) => {
            if let Some(value) = $value {
                if let Some(existing) = owners.get($key) {
                    return Err(Error::new(
                        codes::INCLUDE_002,
                        format!(
                            "section `{}` is defined in both {} and {}; each top-level section must have exactly one owner",
                            $key,
                            existing.display(),
                            path.display()
                        ),
                    )
                    .at("include"));
                }
                merged.$field = Some(value);
                owners.insert($key.to_string(), path.to_path_buf());
            }
        };
    }
    set!(package, "package", fragment.package);
    set!(vars, "vars", fragment.vars);
    set!(build, "build", fragment.build);
    set!(sources, "sources", fragment.sources);
    set!(patches, "patches", fragment.patches);
    set!(source_assembly, "source-assembly", fragment.source_assembly);
    set!(matrix, "matrix", fragment.matrix);
    set!(outputs, "outputs", fragment.outputs);
    set!(tests, "tests", fragment.tests);
    Ok(())
}

fn display(root_dir: &Path, path: &Path) -> String {
    path.strip_prefix(root_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::load_recipe_bundle;
    use rpmx_base::error::codes;

    fn write(path: &std::path::Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, text).expect("write");
    }

    const PACKAGE: &str = "schema: 1\npackage:\n  name: probe\n  version: \"1.0\"\n  release: 1\n  summary: probe\n  license: MIT\n";

    #[test]
    fn a_single_file_is_a_bundle() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("package.yaml");
        write(&path, PACKAGE);
        let bundle = load_recipe_bundle(&path).expect("bundle");
        assert_eq!(bundle.recipe.package.name, "probe");
        assert_eq!(bundle.files.len(), 1);
        assert_eq!(
            bundle.owner("package"),
            Some(path.canonicalize().unwrap().as_path())
        );
    }

    #[test]
    fn sections_can_live_in_their_own_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path().join("package.yaml");
        write(
            &root,
            "schema: 1\ninclude: [build.yaml, outputs.yaml]\npackage:\n  name: probe\n  version: \"1.0\"\n  release: 1\n  summary: probe\n  license: MIT\n",
        );
        write(
            &directory.path().join("build.yaml"),
            "build:\n  pipeline:\n    - runs: \"true\"\n",
        );
        write(
            &directory.path().join("outputs.yaml"),
            "outputs:\n  - name: probe\n    metapackage: true\n",
        );
        let bundle = load_recipe_bundle(&root).expect("bundle");
        assert_eq!(bundle.recipe.build.pipeline.len(), 1);
        assert_eq!(bundle.recipe.outputs.len(), 1);
        assert_eq!(bundle.files.len(), 3);
        assert!(bundle.digest().starts_with("sha512:"));
    }

    #[test]
    fn a_duplicate_section_names_both_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path().join("package.yaml");
        write(
            &root,
            &format!("{PACKAGE}include: [build.yaml]\nbuild:\n  pipeline: []\n"),
        );
        write(
            &directory.path().join("build.yaml"),
            "build:\n  pipeline:\n    - runs: \"true\"\n",
        );
        let error = load_recipe_bundle(&root).expect_err("duplicate");
        assert_eq!(error.code, codes::INCLUDE_002);
        assert!(error.message.contains("package.yaml"), "{}", error.message);
        assert!(error.message.contains("build.yaml"), "{}", error.message);
    }

    #[test]
    fn missing_cycles_and_escapes_are_diagnosed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path().join("package.yaml");
        write(&root, &format!("{PACKAGE}include: [missing.yaml]\n"));
        let error = load_recipe_bundle(&root).expect_err("missing");
        assert_eq!(error.code, codes::INCLUDE_001);

        write(&root, &format!("{PACKAGE}include: [a.yaml]\n"));
        write(
            &directory.path().join("a.yaml"),
            "include: [package.yaml]\n",
        );
        let error = load_recipe_bundle(&root).expect_err("cycle");
        assert_eq!(error.code, codes::INCLUDE_005);

        let outer = tempfile::tempdir().expect("outer");
        let recipe_dir = outer.path().join("pkg");
        let root = recipe_dir.join("package.yaml");
        write(&root, &format!("{PACKAGE}include: [../outside.yaml]\n"));
        let outside = outer.path().join("outside.yaml");
        write(&outside, "build:\n  pipeline: []\n");
        let error = load_recipe_bundle(&root).expect_err("escape");
        assert_eq!(error.code, codes::INCLUDE_004);
    }

    #[test]
    fn a_fragment_must_not_declare_a_schema() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path().join("package.yaml");
        write(&root, &format!("{PACKAGE}include: [build.yaml]\n"));
        write(
            &directory.path().join("build.yaml"),
            "schema: 1\nbuild:\n  pipeline: []\n",
        );
        let error = load_recipe_bundle(&root).expect_err("fragment schema");
        assert_eq!(error.code, codes::INCLUDE_006);
    }
}
