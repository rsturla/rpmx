//! The reviewable package lock.
//!
//! A lock records what the recipe resolved to: source digests, step digests,
//! the expanded cells, policy, and the root requirement. `build --locked`
//! refuses to run when any of those changed since the lock was written.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use rpmx_core::error::codes;
use rpmx_core::plan::Plan;
use rpmx_core::Error;
use rpmx_core::RecipeBundle;

use crate::commands::{
    autoroot, load_plan, resolve_policy, source_date_epoch, LockArgs, PlanPolicy, PolicyInput,
};

pub const FILE: &str = "package.lock.json";
pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLock {
    pub schema: u32,
    pub recipe_digest: String,
    #[serde(default)]
    pub recipe_files: Vec<LockedRecipeFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<LockedContext>,
    pub source_date_epoch: u32,
    pub policy: LockedPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<LockedRoot>,
    /// Target sysroot for a cross build. Without it `--locked` would pin only
    /// half of what the build linked against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sysroot: Option<LockedRoot>,
    pub sources: Vec<LockedSource>,
    pub cells: Vec<LockedCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedRecipeFile {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedContext {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dist_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_release: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedRoot {
    pub profile: String,
    pub requirement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closure_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedSource {
    pub name: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedCell {
    pub id: String,
    pub package: String,
    pub version: String,
    pub release: String,
    pub outputs: Vec<String>,
    pub steps: Vec<LockedStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedStep {
    pub scope: String,
    #[serde(default)]
    pub label: String,
    pub uses: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

pub fn run(args: LockArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    let declared = rpmx_core::load_recipe_bundle(&args.file)?;
    let arch = match &args.arch {
        Some(arch) => arch.clone(),
        None => crate::commands::autoroot::declared_arch(
            &declared.recipe,
            &declared.root,
            args.policy.profile.as_deref(),
        )?
        .unwrap_or_else(|| rpmx_core::arch::native_arch().to_string()),
    };
    let (bundle, plan) = load_plan(&args.file, &arch, pipeline_dir, false, true, policy)?;
    let lock = build(
        &bundle,
        &plan,
        policy,
        args.policy.profile.as_deref(),
        &arch,
        None,
        args.root_lock
            .as_deref()
            .map(rpmx_core::root::load_lock)
            .transpose()?
            .as_ref(),
        None,
    )?;
    let path = args
        .output
        .clone()
        .unwrap_or_else(|| default_path(&args.file));

    let unpinned = lock
        .root
        .as_ref()
        .is_some_and(|root| root.root_digest.is_none())
        || lock
            .sysroot
            .as_ref()
            .is_some_and(|root| root.root_digest.is_none());
    if unpinned {
        rpmx_core::output::warning(
            "lock",
            "a root is not assembled yet, so its digest is not pinned; \
             run a build and `rpmx lock` again to pin it",
        );
    }
    if args.check {
        let mut existing = read(&path)?;
        adopt_unpinned(&mut existing, &lock);
        if existing != lock {
            return Err(Error::new(
                codes::LOCK_002,
                format!(
                    "{} is out of date ({}); run `rpmx lock` to update it",
                    path.display(),
                    differences(&existing, &lock)
                ),
            )
            .at(path.display().to_string()));
        }
        println!("lock is up to date: {}", path.display());
        return Ok(());
    }

    if args.output.as_deref() == Some(Path::new("-")) {
        println!("{}", serde_json::to_string_pretty(&lock).map_err(encode)?);
    } else {
        write(&path, &lock)?;
        println!("{}", path.display());
    }
    Ok(())
}

/// Verify that a recipe and plan still match the lock next to the recipe.
#[allow(clippy::too_many_arguments)]
pub fn verify(
    bundle: &RecipeBundle,
    plan: &Plan,
    policy: PlanPolicy<'_>,
    profile: Option<&Path>,
    arch: &str,
    explicit_source_date_epoch: Option<u32>,
    active_root: Option<&rpmx_core::root::RootLock>,
    active_sysroot: Option<&rpmx_core::root::RootLock>,
    check_root: bool,
) -> Result<(), Error> {
    let path = default_path(&bundle.root);
    if !path.is_file() {
        return Err(Error::new(
            codes::LOCK_003,
            format!(
                "no package lock at {}; run `rpmx lock` first or drop --locked",
                path.display()
            ),
        )
        .at(path.display().to_string()));
    }
    let existing = read(&path)?;
    let actual = build(
        bundle,
        plan,
        policy,
        profile,
        arch,
        explicit_source_date_epoch,
        if check_root { active_root } else { None },
        if check_root { active_sysroot } else { None },
    )?;
    if check_root && existing.root.is_some() && active_root.is_none() {
        return Err(Error::new(
            codes::LOCK_002,
            format!(
                "{} pins a build root but no root is active; pass --root-lock and --root",
                path.display()
            ),
        )
        .at(path.display().to_string()));
    }
    let unchanged = if check_root {
        let mut expected = existing.clone();
        {
            adopt_unpinned(&mut expected, &actual);
        }
        expected == actual
    } else {
        let mut expected = existing.clone();
        let mut observed = actual.clone();
        expected.root = None;
        observed.root = None;
        expected == observed
    };
    if !unchanged {
        return Err(Error::new(
            codes::LOCK_002,
            format!(
                "{} has drifted ({}); run `rpmx lock` and review the diff",
                path.display(),
                differences(&existing, &actual)
            ),
        )
        .at(path.display().to_string()));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn build(
    bundle: &RecipeBundle,
    plan: &Plan,
    policy: PlanPolicy<'_>,
    profile: Option<&Path>,
    arch: &str,
    explicit_source_date_epoch: Option<u32>,
    active_root: Option<&rpmx_core::root::RootLock>,
    active_sysroot: Option<&rpmx_core::root::RootLock>,
) -> Result<PackageLock, Error> {
    let (root, sysroot) =
        match autoroot::requirement(&bundle.recipe, plan, &bundle.root, profile, arch)? {
            Some(requirement) => {
                let resolved = active_root
                    .cloned()
                    .or_else(|| autoroot::cached_lock_for_requirement(&requirement.digest));
                let root = LockedRoot {
                    profile: requirement.name.clone(),
                    requirement: requirement.digest.clone(),
                    arch: Some(arch.to_string()),
                    closure_digest: resolved.as_ref().map(|lock| lock.closure_digest.clone()),
                    root_digest: resolved.as_ref().and_then(|lock| lock.root_digest.clone()),
                };
                let sysroot = requirement.sysroot.as_ref().map(|sysroot| {
                    let resolved = active_sysroot
                        .cloned()
                        .or_else(|| autoroot::cached_lock_for_requirement(&sysroot.digest));
                    LockedRoot {
                        profile: sysroot.name.clone(),
                        requirement: sysroot.digest.clone(),
                        arch: Some(arch.to_string()),
                        closure_digest: resolved.as_ref().map(|lock| lock.closure_digest.clone()),
                        root_digest: resolved.as_ref().and_then(|lock| lock.root_digest.clone()),
                    }
                });
                (Some(root), sysroot)
            }
            // An explicitly supplied root is a lock input even when the recipe
            // does not describe one.
            None => (
                active_root.map(|lock| LockedRoot {
                    profile: lock.profile.clone(),
                    requirement: String::new(),
                    arch: Some(arch.to_string()),
                    closure_digest: Some(lock.closure_digest.clone()),
                    root_digest: lock.root_digest.clone(),
                }),
                None,
            ),
        };
    let cells = plan
        .cells
        .iter()
        .map(|cell| LockedCell {
            id: cell.id.clone(),
            package: cell.package.clone(),
            version: cell.version.clone(),
            release: cell.release.clone(),
            outputs: cell
                .outputs
                .iter()
                .map(|output| output.name.clone())
                .collect(),
            steps: plan
                .source_assembly
                .iter()
                .flat_map(|source| {
                    source.steps.iter().map(|step| LockedStep {
                        scope: "source-assembly".to_string(),
                        label: step.label.clone(),
                        uses: step.uses.clone(),
                        digest: step.step_digest.clone(),
                    })
                })
                .chain(cell.pipeline.iter().map(|step| LockedStep {
                    scope: "build".to_string(),
                    label: step.label.clone(),
                    uses: step.uses.clone(),
                    digest: step.step_digest.clone(),
                }))
                .chain(cell.tests.iter().flat_map(|test| {
                    test.pipeline.iter().map(move |step| LockedStep {
                        scope: format!("test:{}", test.name),
                        label: step.label.clone(),
                        uses: step.uses.clone(),
                        digest: step.step_digest.clone(),
                    })
                }))
                .collect(),
        })
        .collect();
    Ok(PackageLock {
        schema: SCHEMA,
        recipe_digest: bundle.digest().to_string(),
        recipe_files: bundle
            .files
            .iter()
            .map(|file| LockedRecipeFile {
                path: file.name.clone(),
                digest: file.digest.clone(),
            })
            .collect(),
        context: locked_context(bundle)?,
        source_date_epoch: source_date_epoch(&bundle.recipe, explicit_source_date_epoch),
        policy: LockedPolicy {
            dist_tag: policy.dist_tag.map(str::to_string),
            build_release: policy.build_release,
        },
        root,
        sysroot,
        sources: bundle
            .recipe
            .sources
            .iter()
            .map(|source| LockedSource {
                name: source.name.clone(),
                digest: source.digest.clone(),
            })
            .collect(),
        cells,
    })
}

fn default_path(recipe: &Path) -> PathBuf {
    recipe.parent().unwrap_or_else(|| Path::new(".")).join(FILE)
}

fn locked_context(bundle: &RecipeBundle) -> Result<Option<LockedContext>, Error> {
    let Some(path) = &bundle.recipe.context else {
        return Ok(None);
    };
    let recipe_dir = bundle.root.parent().unwrap_or_else(|| Path::new("."));
    let root = rpmx_core::source::context_root(recipe_dir, path)?;
    let digest = rpmx_core::tree::context_digest(&root)?;
    Ok(Some(LockedContext {
        path: path.clone(),
        digest,
    }))
}

fn read(path: &Path) -> Result<PackageLock, Error> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::LOCK_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let lock: PackageLock = serde_json::from_str(&text).map_err(|error| {
        Error::new(
            codes::LOCK_001,
            format!("invalid {}: {error}", path.display()),
        )
    })?;
    if lock.schema != SCHEMA {
        return Err(Error::new(
            codes::LOCK_001,
            format!(
                "unsupported lock schema {} in {}; this rpmx writes {SCHEMA}",
                lock.schema,
                path.display()
            ),
        ));
    }
    Ok(lock)
}

fn write(path: &Path, lock: &PackageLock) -> Result<(), Error> {
    let mut text = serde_json::to_string_pretty(lock).map_err(encode)?;
    text.push('\n');
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text).map_err(|error| {
        Error::new(
            codes::LOCK_001,
            format!("cannot write {}: {error}", temporary.display()),
        )
    })?;
    std::fs::rename(&temporary, path).map_err(|error| {
        Error::new(
            codes::LOCK_001,
            format!("cannot replace {}: {error}", path.display()),
        )
    })
}

/// Copy digests into a lock that never recorded them.
///
/// `rpmx lock` on a machine with a cold cache can only pin the requirement,
/// not the closure. Comparing that `null` against a resolved digest would
/// report drift for a root that never claimed one, so the missing side adopts
/// the resolved value; a genuinely different digest still differs.
fn adopt_unpinned(expected: &mut PackageLock, observed: &PackageLock) {
    for (expected, observed) in [
        (&mut expected.root, observed.root.as_ref()),
        (&mut expected.sysroot, observed.sysroot.as_ref()),
    ] {
        let (Some(expected), Some(observed)) = (expected.as_mut(), observed) else {
            continue;
        };
        if expected.closure_digest.is_none() {
            expected.closure_digest = observed.closure_digest.clone();
        }
        if expected.root_digest.is_none() {
            expected.root_digest = observed.root_digest.clone();
        }
    }
}

fn differences(old: &PackageLock, new: &PackageLock) -> String {
    let mut fields = Vec::new();
    if old.recipe_digest != new.recipe_digest {
        fields.push("recipe");
    }
    if old.recipe_files != new.recipe_files {
        fields.push("recipe-files");
    }
    if old.context != new.context {
        fields.push("context");
    }
    if old.source_date_epoch != new.source_date_epoch {
        fields.push("source-date-epoch");
    }
    if old.policy != new.policy {
        fields.push("policy");
    }
    if old.root != new.root {
        fields.push("root");
    }
    if old.sysroot != new.sysroot {
        fields.push("sysroot");
    }
    if old.sources != new.sources {
        fields.push("sources");
    }
    if old.cells != new.cells {
        fields.push("cells");
    }
    if fields.is_empty() {
        "unknown difference".to_string()
    } else {
        format!("changed: {}", fields.join(", "))
    }
}

fn encode(error: serde_json::Error) -> Error {
    Error::new(codes::LOCK_001, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{adopt_unpinned, build, LockedRoot, PackageLock, SCHEMA};
    use crate::commands::PlanPolicy;
    use std::fs;

    const RECIPE: &str = r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: 1
  summary: probe
  license: MIT
  source-date-epoch: 1704067200
sources:
  - name: probe.tar.gz
    kind: archive
    digest: "sha512:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    locators:
      - kind: https
        url: https://example.invalid/probe.tar.gz
build:
  pipeline:
    - uses: strip
outputs:
  - name: probe
    metapackage: true
"#;

    fn fixture(path: &std::path::Path) -> PackageLock {
        std::fs::write(path, RECIPE).expect("recipe");
        let bundle = rpmx_core::load_recipe_bundle(path).expect("bundle");
        let plan = rpmx_core::plan_with_options(
            &bundle.recipe,
            rpmx_core::PlanOptions {
                arch: "x86_64",
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan");
        build(
            &bundle,
            &plan,
            PlanPolicy::default(),
            None,
            "x86_64",
            None,
            None,
            None,
        )
        .expect("lock")
    }

    #[test]
    fn an_unpinned_root_digest_is_adopted_and_a_real_difference_is_not() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut lock = fixture(&directory.path().join("package.yaml"));
        let root = |arch: &str| LockedRoot {
            profile: format!("{arch}-root"),
            requirement: format!("sha512:{arch}"),
            arch: Some(arch.to_string()),
            closure_digest: Some("sha512:closure".to_string()),
            root_digest: Some("sha512:root".to_string()),
        };
        lock.root = Some(root("x86_64"));
        lock.sysroot = Some(root("aarch64"));

        // A lock written with a cold cache has the requirement but no digest.
        let mut unpinned = lock.clone();
        for entry in [unpinned.root.as_mut(), unpinned.sysroot.as_mut()]
            .into_iter()
            .flatten()
        {
            entry.closure_digest = None;
            entry.root_digest = None;
        }
        adopt_unpinned(&mut unpinned, &lock);
        assert_eq!(unpinned, lock, "missing digests are not drift");

        // A digest that was recorded and differs is still drift.
        let mut drifted = lock.clone();
        drifted.root.as_mut().expect("root").root_digest = Some("sha512:other".to_string());
        let mut adopted = drifted.clone();
        adopt_unpinned(&mut adopted, &lock);
        assert_ne!(adopted, lock);
    }

    #[test]
    fn a_lock_records_the_schema_recipe_sources_and_steps() {
        let directory = tempfile::tempdir().expect("tempdir");
        let lock = fixture(&directory.path().join("package.yaml"));
        assert_eq!(lock.schema, SCHEMA);
        assert!(lock.recipe_digest.starts_with("sha512:"));
        assert_eq!(lock.source_date_epoch, 1_704_067_200);
        assert_eq!(lock.sources.len(), 1);
        assert_eq!(lock.cells.len(), 1);
        assert_eq!(lock.cells[0].package, "probe");
        assert_eq!(lock.cells[0].outputs, ["probe"]);
        assert!(
            lock.cells[0].steps.iter().any(|step| step.uses == "strip"),
            "step missing from lock"
        );
    }

    #[test]
    fn locks_written_before_recipe_files_still_deserialize() {
        let lock = serde_json::json!({
            "schema": 1,
            "recipe_digest": "sha512:old",
            "source_date_epoch": 0,
            "policy": {},
            "sources": [],
            "cells": []
        });
        let parsed: PackageLock = serde_json::from_value(lock).expect("old lock");
        assert!(parsed.recipe_files.is_empty());
    }

    #[test]
    fn a_lock_is_deterministic() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("package.yaml");
        let first = fixture(&path);
        let bundle = rpmx_core::load_recipe_bundle(&path).expect("bundle");
        let plan = rpmx_core::plan_with_options(
            &bundle.recipe,
            rpmx_core::PlanOptions {
                arch: "x86_64",
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan");
        let second = build(
            &bundle,
            &plan,
            PlanPolicy::default(),
            None,
            "x86_64",
            None,
            None,
            None,
        )
        .expect("lock");
        assert_eq!(first, second);
    }

    #[test]
    fn a_lock_records_the_context_tree_digest() {
        let directory = tempfile::tempdir().expect("tempdir");
        let context = directory.path().join("files");
        fs::create_dir_all(&context).expect("context");
        fs::write(context.join("os-release"), b"ID=probe\n").expect("context file");

        let path = directory.path().join("package.yaml");
        let recipe = RECIPE.replace("build:", "context: files\nbuild:");
        fs::write(&path, recipe).expect("recipe");

        let lock = plan_and_build(&path);
        let first = lock.context.clone().expect("context lock");
        assert_eq!(first.path, "files");
        assert!(first.digest.starts_with("sha512:"));

        fs::write(context.join("os-release"), b"ID=changed\n").expect("context file");
        let second = plan_and_build(&path).context.expect("context lock");
        assert_ne!(first.digest, second.digest);
    }

    fn plan_and_build(path: &std::path::Path) -> PackageLock {
        let bundle = rpmx_core::load_recipe_bundle(path).expect("bundle");
        let plan = rpmx_core::plan_with_options(
            &bundle.recipe,
            rpmx_core::PlanOptions {
                arch: "x86_64",
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan");
        build(
            &bundle,
            &plan,
            PlanPolicy::default(),
            None,
            "x86_64",
            None,
            None,
            None,
        )
        .expect("lock")
    }
}
