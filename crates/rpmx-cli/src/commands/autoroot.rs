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
    RootSelection,
};
use rpmx_core::tree::digest_root;
use rpmx_core::Error;

use crate::commands::fetch::default_cache;

/// A root that was resolved or found in the cache.
#[derive(Debug)]
pub struct AutomaticRoot {
    pub lock: PathBuf,
    pub root: PathBuf,
    pub document: RootLock,
    pub profile: String,
    pub packages: usize,
    pub cached: bool,
}

/// The merged root requirement for a recipe: profile, name and digest.
pub struct Requirement {
    pub profile: Profile,
    pub name: String,
    pub digest: String,
}

/// Build the merged root requirement without touching the network or cache.
///
/// Returns `None` when the recipe names no profile and the caller gave none.
pub fn requirement(
    recipe: &Recipe,
    plan: &Plan,
    recipe_path: &Path,
    profile: Option<&Path>,
    arch: &str,
) -> Result<Option<Requirement>, Error> {
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
    // Inline roots never pass through `load_profile`; validate the assembled
    // profile before anything derives an architecture or a root path from it.
    profile.validate()?;

    let fallback = format!("{}-root", recipe.package.name);
    let name = profile.name(&fallback);
    let bytes = serde_json::to_vec(&profile)
        .map_err(|error| Error::new(codes::ROOT_006, error.to_string()))?;
    Ok(Some(Requirement {
        profile,
        name,
        digest: sha512_hex(&bytes),
    }))
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
    let Some(requirement) = requirement(recipe, plan, recipe_path, profile, arch)? else {
        return Ok(None);
    };
    let profile = requirement.profile;
    let name = requirement.name;
    let digest = requirement.digest;

    let cas = match cache {
        Some(path) => path.to_path_buf(),
        None => default_cache()?,
    };
    let roots = cas.parent().unwrap_or(&cas).join("roots");
    let key = digest.strip_prefix("sha512:").unwrap_or(&digest);
    let directory = roots.join(key);
    let lock_path = directory.join("root.lock.yaml");
    let root_path = directory.join("root");

    if !refresh {
        match load_cached(&lock_path, &root_path, arch) {
            Ok(Some(root)) => return Ok(Some(root)),
            Ok(None) => {}
            Err(error) if offline => {
                return Err(Error::new(
                    codes::ROOT_016,
                    format!(
                        "cached root {} does not verify ({error}); run without --offline to rebuild it",
                        directory.display()
                    ),
                ));
            }
            Err(_) => {}
        }
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
    // Serialize publishers per requirement. Without this, two cold starts
    // delete each other's half-built root.
    let _publishing = CacheLock::acquire(&roots, key)?;
    if !refresh {
        match load_cached(&lock_path, &root_path, arch) {
            Ok(Some(root)) => return Ok(Some(root)),
            Ok(None) | Err(_) => {}
        }
    }
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

/// Assemble a root described by an SRPM-embedded lock, reusing the cache.
///
/// This is what makes `rpmx rebuild foo.src.rpm` self-sufficient without
/// extracting `root.lock.json` by hand.
pub fn ensure_from_lock(
    lock: &RootLock,
    cache: Option<&Path>,
    offline: bool,
    arch: &str,
) -> Result<PathBuf, Error> {
    let cas = match cache {
        Some(path) => path.to_path_buf(),
        None => default_cache()?,
    };
    let roots = cas.parent().unwrap_or(&cas).join("roots");
    let digest = lock.require_digest()?.to_string();
    let key = digest.strip_prefix("sha512:").unwrap_or(&digest);
    let directory = roots.join(format!("lock-{key}"));
    let lock_path = directory.join("root.lock.yaml");
    let root_path = directory.join("root");

    if let Ok(Some(root)) = load_cached(&lock_path, &root_path, arch) {
        return Ok(root.root);
    }
    if let Some(root) = find_published_root(&roots, &digest) {
        return Ok(root);
    }
    if offline {
        return Err(Error::new(
            codes::ROOT_016,
            format!(
                "the root recorded in this SRPM is not cached at {}; run without --offline to assemble it",
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
    let _publishing = CacheLock::acquire(&roots, &format!("lock-{key}"))?;
    if let Ok(Some(root)) = load_cached(&lock_path, &root_path, arch) {
        return Ok(root.root);
    }
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
    let assembled = assemble(
        lock,
        AssembleOptions {
            cache: &cas,
            work: work.path(),
            root: &root_path,
        },
    )?;
    let actual = digest_root(&assembled)?;
    if actual != digest {
        return Err(Error::new(
            codes::ROOT_017,
            format!("assembled root digest {actual} does not match the SRPM lock's {digest}"),
        ));
    }
    write_lock(&lock_path, lock)?;
    Ok(root_path)
}

/// Find an already-published root with the same digest under a different
/// requirement key. A build's automatic root and an SRPM's embedded lock often
/// describe the same tree.
fn find_published_root(roots: &Path, digest: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(roots).ok()?;
    for entry in entries.flatten() {
        let directory = entry.path();
        let lock_path = directory.join("root.lock.yaml");
        let root_path = directory.join("root");
        if !lock_path.is_file() || !root_path.is_dir() {
            continue;
        }
        let Ok(lock) = load_lock(&lock_path) else {
            continue;
        };
        if lock.root_digest.as_deref() == Some(digest) {
            return std::fs::canonicalize(root_path).ok();
        }
    }
    None
}

fn load_cached(
    lock_path: &Path,
    root_path: &Path,
    arch: &str,
) -> Result<Option<AutomaticRoot>, Error> {
    if !lock_path.is_file() || !root_path.is_dir() {
        return Ok(None);
    }
    let document = load_lock(lock_path)?;
    // Publishing writes the lock last, so a root newer than its lock may have
    // been changed after publication. Re-hash only then: a full walk of a
    // distribution root costs minutes on every build.
    let lock_time = std::fs::metadata(lock_path)
        .and_then(|metadata| metadata.modified())
        .ok();
    let root_time = std::fs::metadata(root_path)
        .and_then(|metadata| metadata.modified())
        .ok();
    let changed = match (lock_time, root_time) {
        (Some(lock), Some(root)) => root > lock,
        _ => true,
    };
    if changed {
        RootSelection::verify(&document, root_path, arch)?;
    }
    let path = std::fs::canonicalize(root_path).map_err(|error| {
        Error::new(
            codes::ROOT_017,
            format!("cannot resolve {}: {error}", root_path.display()),
        )
    })?;
    Ok(Some(AutomaticRoot {
        lock: lock_path.to_path_buf(),
        root: path,
        packages: count_packages(&document),
        profile: document.profile.clone(),
        document,
        cached: true,
    }))
}

/// A per-requirement publishing mutex.
struct CacheLock {
    path: PathBuf,
}

impl CacheLock {
    fn acquire(roots: &Path, key: &str) -> Result<Self, Error> {
        use std::io::Write;

        let path = roots.join(format!("{key}.publishing"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > std::time::Duration::from_secs(900));
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(Error::new(
                            codes::ROOT_004,
                            format!(
                                "timed out waiting for another rpmx process to publish {}",
                                roots.display()
                            ),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(error) => {
                    return Err(Error::new(
                        codes::ROOT_004,
                        format!("cannot create {}: {error}", path.display()),
                    ))
                }
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
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
    // A relative path is relative to the recipe, not the caller's cwd.
    let relative = recipe_dir.join(name);
    if relative.is_file() {
        return Ok(relative);
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

    fn plan(recipe: &Recipe) -> rpmx_core::Plan {
        rpmx_core::plan_with_options(
            recipe,
            rpmx_core::PlanOptions {
                arch: "x86_64",
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan")
    }

    #[test]
    fn inline_roots_are_validated_before_any_resolution() {
        let directory = tempfile::tempdir().expect("tempdir");
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
    releasever: "44"
    archs: [typo]
    repositories:
      - id: repo
        baseurl: https://example.invalid/fedora
    packages: [bash]
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let error = super::ensure(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            Some(directory.path()),
            true,
            false,
            "x86_64",
        )
        .expect_err("invalid arch");
        // `Profile::validate` reports an unknown architecture as ROOT-009.
        assert_eq!(error.code, rpmx_core::error::codes::ROOT_009);
    }

    #[test]
    fn naming_a_profile_and_describing_one_is_rejected() {
        let directory = tempfile::tempdir().expect("tempdir");
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
        let plan = plan(&recipe);
        let error = super::ensure(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            Some(directory.path()),
            true,
            false,
            "x86_64",
        )
        .expect_err("conflicting root");
        assert_eq!(error.code, rpmx_core::error::codes::ROOT_006);
        assert_eq!(error.path.as_deref(), Some("build.root"));
    }

    #[test]
    fn a_profile_path_is_relative_to_the_recipe() {
        let directory = tempfile::tempdir().expect("tempdir");
        let profiles = directory.path().join("profiles");
        std::fs::create_dir_all(&profiles).expect("profiles");
        std::fs::write(
            profiles.join("review.yaml"),
            r#"
document: rpmx-profile
version: 1
name: review
releasever: "44"
archs: [x86_64]
repositories:
  - id: fedora
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
"#,
        )
        .expect("profile");
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
    profile: profiles/review.yaml
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let error = super::ensure(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            Some(directory.path()),
            true,
            false,
            "x86_64",
        )
        .expect_err("cold offline cache");
        assert_eq!(error.code, rpmx_core::error::codes::ROOT_016);
        assert!(
            error.message.contains("not cached"),
            "profile was not discovered: {}",
            error.message
        );
    }

    #[test]
    fn publishing_the_same_requirement_is_serialized() {
        let directory = tempfile::tempdir().expect("tempdir");
        let held = super::CacheLock::acquire(directory.path(), "key").expect("first lock");
        let path = directory.path().to_path_buf();
        let started = std::time::Instant::now();
        let waiter = std::thread::spawn(move || {
            let _second = super::CacheLock::acquire(&path, "key").expect("second lock");
            started.elapsed()
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        let waiting = waiter.is_finished();
        drop(held);
        let elapsed = waiter.join().expect("thread");
        assert!(!waiting, "second publisher did not wait");
        assert!(
            elapsed >= std::time::Duration::from_millis(150),
            "second publisher returned after {elapsed:?}"
        );
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
