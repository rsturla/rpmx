use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::error::{codes, Error, Result};
use crate::model::Recipe;
use crate::plan::{Plan, PlannedStep};
use crate::root::RootLock;
use crate::srpm::{self, LockStep};
use crate::steps;

#[derive(Debug, Clone)]
pub struct PreparedSourceFile {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
}

#[derive(Debug, Clone)]
pub struct PreparedSourcePackage {
    pub name: String,
    pub version: String,
    pub release: String,
    pub epoch: Option<u32>,
    pub summary: String,
    pub license: String,
    pub description: String,
    pub files: Vec<PreparedSourceFile>,
}

pub struct SourcePackageOptions<'a> {
    pub recipe: &'a Recipe,
    pub plan: &'a Plan,
    pub recipe_bytes: &'a [u8],
    pub step_root: Option<&'a Path>,
    pub sources: &'a Path,
    pub source_date_epoch: u32,
    pub root_lock: Option<&'a RootLock>,
    pub debuginfo: bool,
    pub sbom: bool,
}

pub fn prepare_source_packages(
    options: SourcePackageOptions<'_>,
) -> Result<Vec<PreparedSourcePackage>> {
    let snapshot = srpm::snapshot_dir(options.sources)?;
    for source in &snapshot {
        if matches!(
            source.path.as_str(),
            "package.yaml"
                | "package.lock.json"
                | "RPMX-MANIFEST.json"
                | "RPMX-STEPS.json"
                | "root.lock.json"
        ) {
            return Err(Error::new(
                codes::SRPM_007,
                format!("source file '{}' uses a reserved SRPM name", source.path),
            ));
        }
    }

    let recipe_digest = srpm::sha512_hex(options.recipe_bytes);
    let (steps, step_files) = collect_local_steps(options.plan, options.step_root)?;
    let root_lock = match options.root_lock {
        Some(lock) => {
            let mut text = serde_json::to_string_pretty(lock)
                .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?;
            text.push('\n');
            let digest = srpm::sha512_hex(text.as_bytes());
            Some((
                text,
                srpm::LockRoot {
                    path: "root.lock.json".to_string(),
                    digest,
                },
            ))
        }
        None => None,
    };

    let mut packages = Vec::with_capacity(options.plan.cells.len());
    for cell in &options.plan.cells {
        let build = srpm::LockBuild {
            tools: cell.tools.clone(),
            host: cell.host.clone(),
            target: cell.target.clone(),
            tests: cell.test_tools.clone(),
            debuginfo: options.debuginfo,
            sbom: options.sbom,
        };
        let lock_root = root_lock.as_ref().map(|(_, reference)| reference);
        let lock = srpm::lock_json(
            &recipe_digest,
            &cell.version,
            &cell.release,
            options.plan.dist_tag.as_deref(),
            options.plan.build_release,
            options.source_date_epoch,
            &snapshot,
            &steps,
            srpm::LockBuildRef {
                root_lock: lock_root,
                build: &build,
            },
        )?;
        let lock_digest = srpm::sha512_hex(lock.as_bytes());
        let manifest = srpm::manifest_json(
            &recipe_digest,
            &lock_digest,
            options.source_date_epoch,
            &snapshot,
            &steps,
            lock_root,
        )?;

        let mut files = vec![
            PreparedSourceFile {
                path: "package.yaml".to_string(),
                data: options.recipe_bytes.to_vec(),
                mode: 0o644,
            },
            PreparedSourceFile {
                path: "package.lock.json".to_string(),
                data: lock.into_bytes(),
                mode: 0o644,
            },
            PreparedSourceFile {
                path: "RPMX-MANIFEST.json".to_string(),
                data: manifest.into_bytes(),
                mode: 0o644,
            },
        ];
        files.extend(step_files.iter().cloned());
        if let Some((text, _)) = &root_lock {
            files.push(PreparedSourceFile {
                path: "root.lock.json".to_string(),
                data: text.as_bytes().to_vec(),
                mode: 0o644,
            });
        }
        for source in &snapshot {
            files.push(PreparedSourceFile {
                path: source.path.clone(),
                data: source.data.clone(),
                mode: source.mode,
            });
        }

        packages.push(PreparedSourcePackage {
            name: cell.package.clone(),
            version: cell.version.clone(),
            release: cell.release.clone(),
            epoch: options.recipe.package.epoch,
            summary: options.recipe.package.summary.clone(),
            license: options.recipe.package.license.clone(),
            description: options
                .recipe
                .package
                .description
                .clone()
                .unwrap_or_else(|| options.recipe.package.summary.clone()),
            files,
        });
    }
    Ok(packages)
}

fn collect_local_steps(
    plan: &Plan,
    step_root: Option<&Path>,
) -> Result<(Vec<LockStep>, Vec<PreparedSourceFile>)> {
    let mut refs: Vec<&PlannedStep> = Vec::new();
    if let Some(source) = &plan.source_assembly {
        refs.extend(source.steps.iter());
    }
    for cell in &plan.cells {
        refs.extend(cell.pipeline.iter());
        for test in &cell.tests {
            refs.extend(test.pipeline.iter());
        }
    }

    let mut seen = BTreeSet::new();
    let mut steps = Vec::new();
    let mut contents = BTreeMap::new();
    for step in refs {
        let Some(digest) = &step.step_digest else {
            continue;
        };
        if !seen.insert(step.uses.clone()) {
            continue;
        }
        steps.push(LockStep {
            name: step.uses.clone(),
            digest: digest.clone(),
        });
        if step.builtin {
            let text = steps::builtin_source(&step.uses).ok_or_else(|| {
                Error::new(
                    codes::SRPM_010,
                    format!("built-in step '{}' is not embedded in this rpmx", step.uses),
                )
            })?;
            let actual = srpm::sha512_hex(text.as_bytes());
            if &actual != digest {
                return Err(Error::new(
                    codes::SRPM_009,
                    format!(
                        "built-in step '{}' changed (expected {digest}, got {actual})",
                        step.uses
                    ),
                ));
            }
            contents.insert(step.uses.clone(), text.to_string());
            continue;
        }
        let root = step_root.ok_or_else(|| {
            Error::new(
                codes::SRPM_003,
                format!("no pipeline directory for local step '{}'", step.uses),
            )
        })?;
        let path = root.join(format!("{}.yaml", step.uses));
        let data = std::fs::read(&path).map_err(|error| {
            Error::new(
                codes::SRPM_004,
                format!("cannot read {}: {error}", path.display()),
            )
        })?;
        let actual = srpm::sha512_hex(&data);
        if &actual != digest {
            return Err(Error::new(
                codes::SRPM_009,
                format!(
                    "step '{}' changed after planning (expected {digest}, got {actual})",
                    step.uses
                ),
            ));
        }
        let text = String::from_utf8(data).map_err(|error| {
            Error::new(
                codes::SRPM_005,
                format!("step {} is not UTF-8: {error}", path.display()),
            )
        })?;
        contents.insert(step.uses.clone(), text);
    }

    let mut files = Vec::new();
    if !contents.is_empty() {
        let mut text = serde_json::to_string_pretty(&contents)
            .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?;
        text.push('\n');
        files.push(PreparedSourceFile {
            path: "RPMX-STEPS.json".to_string(),
            data: text.into_bytes(),
            mode: 0o644,
        });
    }
    Ok((steps, files))
}
