//! The reviewable package lock.
//!
//! A lock records what the recipe resolved to: source digests, step digests,
//! the expanded cells, policy, and the root requirement. `build --locked`
//! refuses to run when any of those changed since the lock was written.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use rpmx_core::error::codes;
use rpmx_core::hash::sha512_hex;
use rpmx_core::model::Recipe;
use rpmx_core::plan::Plan;
use rpmx_core::Error;

use crate::commands::{
    autoroot, load_plan, resolve_policy, source_date_epoch, LockArgs, PlanPolicy, PolicyInput,
};

pub const FILE: &str = "package.lock.json";
pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageLock {
    pub schema: u32,
    pub recipe_digest: String,
    pub source_date_epoch: u32,
    pub policy: LockedPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<LockedRoot>,
    pub sources: Vec<LockedSource>,
    pub cells: Vec<LockedCell>,
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
    let (recipe, plan) = load_plan(&args.file, &args.arch, pipeline_dir, false, policy)?;
    let lock = build(
        &recipe,
        &plan,
        &args.file,
        policy,
        args.policy.profile.as_deref(),
        &args.arch,
        None,
        args.root_lock
            .as_deref()
            .map(rpmx_core::root::load_lock)
            .transpose()?
            .as_ref(),
    )?;
    let path = args
        .output
        .clone()
        .unwrap_or_else(|| default_path(&args.file));

    if args.check {
        let existing = read(&path)?;
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
pub fn verify(
    recipe: &Recipe,
    plan: &Plan,
    recipe_path: &Path,
    policy: PlanPolicy<'_>,
    profile: Option<&Path>,
    arch: &str,
    explicit_source_date_epoch: Option<u32>,
    active_root: Option<&rpmx_core::root::RootLock>,
) -> Result<(), Error> {
    let path = default_path(recipe_path);
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
        recipe,
        plan,
        recipe_path,
        policy,
        profile,
        arch,
        explicit_source_date_epoch,
        active_root,
    )?;
    if existing != actual {
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

pub fn build(
    recipe: &Recipe,
    plan: &Plan,
    recipe_path: &Path,
    policy: PlanPolicy<'_>,
    profile: Option<&Path>,
    arch: &str,
    explicit_source_date_epoch: Option<u32>,
    active_root: Option<&rpmx_core::root::RootLock>,
) -> Result<PackageLock, Error> {
    let bytes = std::fs::read(recipe_path).map_err(|error| {
        Error::new(
            codes::IO_001,
            format!("cannot read {}: {error}", recipe_path.display()),
        )
    })?;
    let root = autoroot::requirement(recipe, plan, recipe_path, profile, arch)?.map(|root| {
        let resolved = active_root
            .cloned()
            .or_else(|| autoroot::cached_lock_for_requirement(&root.digest));
        LockedRoot {
            profile: root.name,
            requirement: root.digest,
            arch: Some(arch.to_string()),
            closure_digest: resolved.as_ref().map(|lock| lock.closure_digest.clone()),
            root_digest: resolved.as_ref().and_then(|lock| lock.root_digest.clone()),
        }
    });
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
                        uses: step.uses.clone(),
                        digest: step.step_digest.clone(),
                    })
                })
                .chain(cell.pipeline.iter().map(|step| LockedStep {
                    scope: "build".to_string(),
                    uses: step.uses.clone(),
                    digest: step.step_digest.clone(),
                }))
                .chain(cell.tests.iter().flat_map(|test| {
                    test.pipeline.iter().map(move |step| LockedStep {
                        scope: format!("test:{}", test.name),
                        uses: step.uses.clone(),
                        digest: step.step_digest.clone(),
                    })
                }))
                .collect(),
        })
        .collect();
    Ok(PackageLock {
        schema: SCHEMA,
        recipe_digest: sha512_hex(&bytes),
        source_date_epoch: source_date_epoch(recipe, explicit_source_date_epoch),
        policy: LockedPolicy {
            dist_tag: policy.dist_tag.map(str::to_string),
            build_release: policy.build_release,
        },
        root,
        sources: recipe
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

fn differences(old: &PackageLock, new: &PackageLock) -> String {
    let mut fields = Vec::new();
    if old.recipe_digest != new.recipe_digest {
        fields.push("recipe");
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
    use super::{build, PackageLock, SCHEMA};
    use crate::commands::PlanPolicy;

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
        let recipe = rpmx_core::load_recipe(path).expect("parse");
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
        build(
            &recipe,
            &plan,
            path,
            PlanPolicy::default(),
            None,
            "x86_64",
            None,
            None,
        )
        .expect("lock")
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
    fn a_lock_is_deterministic() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("package.yaml");
        let first = fixture(&path);
        let recipe = rpmx_core::load_recipe(&path).expect("parse");
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
        let second = build(
            &recipe,
            &plan,
            &path,
            PlanPolicy::default(),
            None,
            "x86_64",
            None,
            None,
        )
        .expect("lock");
        assert_eq!(first, second);
    }
}
