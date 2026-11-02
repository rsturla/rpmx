use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::package_spec::{SourcePackageFile, SourcePackageSpec};
use rpmx_base::error::{codes, Error, Result};
use rpmx_base::guest::CrossToolchain;
use rpmx_language::model::Recipe;
use rpmx_language::plan::{Plan, PlannedStep};
use rpmx_language::steps;
use rpmx_root::root::RootLock;
use rpmx_store::srpm::{self, LockStep};

/// One included recipe fragment embedded next to the root recipe.
#[derive(Debug, Clone)]
pub struct RecipeInclude {
    /// Path relative to the root recipe, using `/`.
    pub name: String,
    pub bytes: Vec<u8>,
}

pub struct SourcePackageOptions<'a> {
    pub recipe: &'a Recipe,
    pub plan: &'a Plan,
    pub recipe_bytes: &'a [u8],
    /// Bundle digest covering the root recipe and every fragment.
    pub recipe_digest: Option<&'a str>,
    pub recipe_includes: &'a [RecipeInclude],
    pub step_root: Option<&'a Path>,
    pub sources: &'a Path,
    pub source_date_epoch: u32,
    pub root_lock: Option<&'a RootLock>,
    /// Target sysroot lock for a cross build.
    pub sysroot_lock: Option<&'a RootLock>,
    /// Toolchain a cross build used, recorded so rebuild needs no profile.
    pub cross: Option<&'a CrossToolchain>,
    pub sbom: bool,
}

pub fn prepare_source_packages(
    options: SourcePackageOptions<'_>,
) -> Result<Vec<SourcePackageSpec>> {
    let snapshot = srpm::snapshot_dir(options.sources)?;
    for source in &snapshot {
        if matches!(
            source.path.as_str(),
            "package.yaml"
                | "package.lock.json"
                | "RPMX-MANIFEST.json"
                | "RPMX-STEPS.json"
                | "root.lock.json"
                | "sysroot.lock.json"
        ) || options
            .recipe_includes
            .iter()
            .any(|include| include.name == source.path)
        {
            return Err(Error::new(
                codes::SRPM_007,
                format!("source file '{}' uses a reserved SRPM name", source.path),
            ));
        }
    }

    let recipe_digest = options
        .recipe_digest
        .map(str::to_string)
        .unwrap_or_else(|| srpm::sha512_hex(options.recipe_bytes));
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

    let sysroot_lock = match options.sysroot_lock {
        Some(lock) => {
            let mut text = serde_json::to_string_pretty(lock)
                .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?;
            text.push('\n');
            let digest = srpm::sha512_hex(text.as_bytes());
            Some((
                text,
                srpm::LockRoot {
                    path: "sysroot.lock.json".to_string(),
                    digest,
                },
            ))
        }
        None => None,
    };
    let cross = match (&sysroot_lock, options.cross) {
        (Some((_, reference)), Some(toolchain)) => Some(srpm::LockCross {
            sysroot: reference.clone(),
            triple: toolchain.triple.clone(),
            cflags: toolchain.cflags.clone(),
            cxxflags: toolchain.cxxflags.clone(),
            ldflags: toolchain.ldflags.clone(),
        }),
        _ => None,
    };

    let mut packages = Vec::with_capacity(options.plan.cells.len());
    for cell in &options.plan.cells {
        let build = srpm::LockBuild {
            tools: cell.tools.clone(),
            host: cell.host.clone(),
            target: cell.target.clone(),
            tests: cell.test_tools.clone(),
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
                cross: cross.as_ref(),
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
            SourcePackageFile {
                path: "package.yaml".to_string(),
                data: options.recipe_bytes.to_vec(),
                mode: 0o644,
            },
            SourcePackageFile {
                path: "package.lock.json".to_string(),
                data: lock.into_bytes(),
                mode: 0o644,
            },
            SourcePackageFile {
                path: "RPMX-MANIFEST.json".to_string(),
                data: manifest.into_bytes(),
                mode: 0o644,
            },
        ];
        files.extend(
            options
                .recipe_includes
                .iter()
                .map(|include| SourcePackageFile {
                    path: include.name.clone(),
                    data: include.bytes.clone(),
                    mode: 0o644,
                }),
        );
        files.extend(step_files.iter().cloned());
        if let Some((text, _)) = &root_lock {
            files.push(SourcePackageFile {
                path: "root.lock.json".to_string(),
                data: text.as_bytes().to_vec(),
                mode: 0o644,
            });
        }
        if let Some((text, _)) = &sysroot_lock {
            files.push(SourcePackageFile {
                path: "sysroot.lock.json".to_string(),
                data: text.as_bytes().to_vec(),
                mode: 0o644,
            });
        }
        for source in &snapshot {
            files.push(SourcePackageFile {
                path: source.path.clone(),
                data: source.data.clone(),
                mode: source.mode,
            });
        }

        packages.push(SourcePackageSpec {
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
            arch: options.plan.arch.clone(),
            source_date_epoch: options.source_date_epoch,
            files,
        });
    }
    Ok(packages)
}

fn collect_local_steps(
    plan: &Plan,
    step_root: Option<&Path>,
) -> Result<(Vec<LockStep>, Vec<SourcePackageFile>)> {
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
        let source = steps::StepSource::read(&path, codes::SRPM_004, codes::SRPM_005)?;
        let actual = source.digest();
        if &actual != digest {
            return Err(Error::new(
                codes::SRPM_009,
                format!(
                    "step '{}' changed after planning (expected {digest}, got {actual})",
                    step.uses
                ),
            ));
        }
        contents.insert(step.uses.clone(), source.into_text());
    }

    let mut files = Vec::new();
    if !contents.is_empty() {
        let mut text = serde_json::to_string_pretty(&contents)
            .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?;
        text.push('\n');
        files.push(SourcePackageFile {
            path: "RPMX-STEPS.json".to_string(),
            data: text.into_bytes(),
            mode: 0o644,
        });
    }
    Ok((steps, files))
}
