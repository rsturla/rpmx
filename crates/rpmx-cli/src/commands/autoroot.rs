//! Resolve, assemble and cache a build root without being asked to.
//!
//! A recipe may name a profile or describe its root inline. The requirement
//! set is the profile's packages plus every dependency the plan declares,
//! including the `needs` of every resolved step. The result is cached by a
//! digest of that requirement, so a second build does no resolution work.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::hash::sha512_hex;
use rpmx_core::model::{Recipe, RootRepository, RootSpec};
use rpmx_core::plan::Plan;
use rpmx_core::profile::{
    load_profile, Builder, Packages, Profile, Repository, ResolveOptions, DOCUMENT, VERSION,
};
use rpmx_core::root::{
    assemble, load_lock, remove_tree, resolve_profile, write_lock, AssembleOptions, RootLock,
};
use rpmx_core::tree::digest_root;
use rpmx_core::Error;

use crate::commands::fetch::default_cache;

/// A root that was resolved or found in the cache.
pub struct AutomaticRoot {
    pub lock: PathBuf,
    pub root: PathBuf,
    pub document: RootLock,
    pub profile: String,
    pub packages: usize,
    pub cached: bool,
}

/// Resolve the automatic root for a recipe, if it declares one.
///
/// Returns `None` when the caller supplied explicit root flags or asked for
/// the host root; the existing explicit-root path handles those.
pub fn ensure(
    recipe: &Recipe,
    plan: &Plan,
    recipe_path: &Path,
    profile: Option<&Path>,
    cache: Option<&Path>,
    offline: bool,
    refresh: bool,
    arch: &str,
) -> Result<Option<AutomaticRoot>, Error> {
    let spec = recipe.build.root.as_ref();
    let selected_profile = profile.map(Path::to_path_buf);
    if spec.is_none() && selected_profile.is_none() {
        return Ok(None);
    }

    let recipe_dir = recipe_path.parent().unwrap_or_else(|| Path::new("."));
    let mut profile = match (spec, selected_profile.as_deref()) {
        (Some(spec), _) if spec.profile.is_some() => {
            if has_inline_fields(spec) {
                return Err(Error::new(
                    codes::ROOT_006,
                    "build.root must name a profile or describe one inline, not both",
                )
                .at("build.root"));
            }
            let path = discover_profile(spec.profile.as_deref().unwrap_or_default(), recipe_dir)?;
            load_profile(&path)?
        }
        (Some(spec), _) => inline_profile(spec, arch)?,
        (None, Some(path)) => load_profile(path)?,
        (None, None) => unreachable!("guarded above"),
    };

    if profile.archs.is_empty() {
        profile.archs.push(arch.to_string());
    }
    let required = required_packages(plan);
    let mut packages: BTreeSet<String> = profile.packages.install.iter().cloned().collect();
    packages.extend(required.iter().cloned());
    profile.packages.install = packages.into_iter().collect();

    let cas = match cache {
        Some(path) => path.to_path_buf(),
        None => default_cache()?,
    };
    let roots = cas.parent().unwrap_or(&cas).join("roots");
    let requirement = serde_json::to_vec(&profile)
        .map_err(|error| Error::new(codes::ROOT_006, error.to_string()))?;
    let digest = sha512_hex(&requirement);
    let key = digest.strip_prefix("sha512:").unwrap_or(&digest);
    let directory = roots.join(key);
    let lock_path = directory.join("root.lock.yaml");
    let root_path = directory.join("root");
    let name = profile.name(recipe.package.name.as_str());

    if !refresh && lock_path.is_file() && root_path.is_dir() {
        // The cache key is the requirement digest and the lock is written
        // last, so a hit is a complete root. Re-hashing every file here costs
        // minutes on a normal distribution root; explicit --root verification
        // remains the way to prove a root on demand.
        let document = load_lock(&lock_path)?;
        let path = std::fs::canonicalize(&root_path).map_err(|error| {
            Error::new(
                codes::ROOT_017,
                format!("cannot resolve {}: {error}", root_path.display()),
            )
        })?;
        return Ok(Some(AutomaticRoot {
            lock: lock_path,
            root: path,
            packages: count_packages(&document),
            profile: document.profile.clone(),
            document,
            cached: true,
        }));
    }

    if offline {
        return Err(Error::new(
            codes::ROOT_016,
            format!(
                "root for '{}' is not cached at {}; run without --offline to resolve it",
                name,
                directory.display()
            ),
        ));
    }

    std::fs::create_dir_all(&roots).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot create {}: {error}", roots.display()),
        )
    })?;
    if directory.exists() {
        remove_tree(&directory)?;
    }
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot create {}: {error}", directory.display()),
        )
    })?;
    let work = tempfile::Builder::new()
        .prefix("rpmx-root-")
        .tempdir_in(&roots)
        .map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!(
                    "cannot create a work directory in {}: {error}",
                    roots.display()
                ),
            )
        })?;
    let mut document = resolve_profile(&profile, &name, work.path(), &cas)?;
    // The final root is a sibling of the scratch directory: assembly refuses
    // a root inside its own work tree.
    let assembled = assemble(
        &document,
        AssembleOptions {
            cache: &cas,
            work: work.path(),
            root: &root_path,
        },
    )?;
    document.root_digest = Some(digest_root(&assembled)?);
    // The lock is written last: a concurrent reader sees either a complete
    // root and lock, or no lock and rebuilds.
    write_lock(&lock_path, &document)?;
    let packages = count_packages(&document);
    Ok(Some(AutomaticRoot {
        lock: lock_path,
        root: root_path,
        document,
        profile: name,
        packages,
        cached: false,
    }))
}

fn has_inline_fields(spec: &RootSpec) -> bool {
    spec.releasever.is_some()
        || !spec.archs.is_empty()
        || !spec.repositories.is_empty()
        || !spec.packages.is_empty()
        || spec.builder.is_some()
}

fn inline_profile(spec: &RootSpec, arch: &str) -> Result<Profile, Error> {
    let releasever = spec.releasever.clone().ok_or_else(|| {
        Error::new(
            codes::ROOT_006,
            "an inline build.root needs releasever, repositories and packages",
        )
        .at("build.root.releasever")
    })?;
    if spec.repositories.is_empty() {
        return Err(Error::new(
            codes::ROOT_006,
            "an inline build.root needs at least one repository",
        )
        .at("build.root.repositories"));
    }
    Ok(Profile {
        document: DOCUMENT.to_string(),
        version: VERSION,
        name: None,
        releasever,
        dist_tag: None,
        archs: if spec.archs.is_empty() {
            vec![arch.to_string()]
        } else {
            spec.archs.clone()
        },
        builder: Builder {
            image: spec.builder.as_ref().map(|builder| builder.image.clone()),
        },
        repositories: spec.repositories.iter().map(repository).collect(),
        packages: Packages {
            install: spec.packages.clone(),
            reinstall: Vec::new(),
        },
        modules: Default::default(),
        options: ResolveOptions::default(),
        debuginfo: false,
        sbom: true,
    })
}

fn repository(source: &RootRepository) -> Repository {
    Repository {
        id: source.id.clone(),
        baseurl: source.baseurl.clone(),
        metalink: source.metalink.clone(),
        mirrorlist: source.mirrorlist.clone(),
        gpgkey: source.gpgkey.clone(),
    }
}

/// Find a profile by path, then by convention.
fn discover_profile(name: &str, recipe_dir: &Path) -> Result<PathBuf, Error> {
    if name.is_empty() {
        return Err(Error::new(codes::ROOT_006, "build.root.profile is empty"));
    }
    let explicit = Path::new(name);
    if explicit.is_file() {
        return Ok(explicit.to_path_buf());
    }
    let mut candidates = vec![
        recipe_dir.join("profiles").join(format!("{name}.yaml")),
        recipe_dir
            .join("profiles")
            .join(format!("{name}.profile.yaml")),
    ];
    if let Some(config) = config_dir() {
        candidates.push(config.join("profiles").join(format!("{name}.yaml")));
    }
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    Err(Error::new(
        codes::ROOT_006,
        format!(
            "profile '{name}' not found; looked at {}",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ))
}

fn config_dir() -> Option<PathBuf> {
    if let Ok(base) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(base).join("rpmx"));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config").join("rpmx"))
}

/// Every package the plan needs in the root.
fn required_packages(plan: &Plan) -> BTreeSet<String> {
    let mut packages = BTreeSet::new();
    if let Some(source) = &plan.source_assembly {
        for step in &source.steps {
            packages.extend(step.needs.iter().cloned());
        }
    }
    for cell in &plan.cells {
        packages.extend(cell.tools.iter().cloned());
        packages.extend(cell.host.iter().cloned());
        packages.extend(cell.target.iter().cloned());
        packages.extend(cell.test_tools.iter().cloned());
        for step in &cell.pipeline {
            packages.extend(step.needs.iter().cloned());
        }
        for test in &cell.tests {
            for step in &test.pipeline {
                packages.extend(step.needs.iter().cloned());
            }
        }
    }
    packages
}

fn count_packages(lock: &RootLock) -> usize {
    lock.packages.values().map(Vec::len).sum()
}

#[cfg(test)]
mod tests {
    use super::{has_inline_fields, required_packages};
    use rpmx_core::model::Recipe;

    fn recipe(text: &str) -> Recipe {
        rpmx_core::parse_recipe(text).expect("recipe")
    }

    #[test]
    fn a_profile_name_and_inline_fields_are_mutually_exclusive() {
        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    profile: base
    releasever: "44"
outputs:
  - name: p
    metapackage: true
"#,
        );
        let spec = recipe.build.root.expect("root");
        assert!(has_inline_fields(&spec));
    }

    #[test]
    fn plan_dependencies_and_step_needs_join_the_root() {
        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  dependencies:
    tools: [gcc]
    host: [zlib-devel]
    tests: [pytest]
  pipeline:
    - uses: strip
outputs:
  - name: p
    metapackage: true
"#,
        );
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
        let packages = required_packages(&plan);
        assert!(packages.contains("gcc"));
        assert!(packages.contains("zlib-devel"));
        assert!(packages.contains("pytest"));
        // A built-in step contributes its own needs.
        assert!(packages.contains("binutils"));
        assert!(packages.contains("findutils"));
    }
}
