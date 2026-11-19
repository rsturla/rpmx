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
    pub mode: u32,
}

pub struct SourcePackageOptions<'a> {
    pub recipe: &'a Recipe,
    pub plan: &'a Plan,
    pub recipe_bytes: &'a [u8],
    pub recipe_mode: u32,
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
    /// Old root identities replaced by a bootstrap rebuild.
    pub replaced: Option<&'a srpm::ReplacedLock>,
}

pub fn prepare_source_packages(
    options: SourcePackageOptions<'_>,
) -> Result<Vec<SourcePackageSpec>> {
    let inventory_path = options.sources.join(rpmx_components::INVENTORY_FILE);
    let inventory_text = if inventory_path.exists() {
        let text = std::fs::read_to_string(&inventory_path).map_err(|error| {
            Error::new(
                codes::SRPM_006,
                format!("cannot read {}: {error}", inventory_path.display()),
            )
        })?;
        rpmx_components::Inventory::from_json(&text)?.to_json()?
    } else {
        rpmx_components::Inventory::empty().to_json()?
    };
    let inventory_digest = srpm::sha512_hex(inventory_text.as_bytes());
    let mut snapshot = srpm::snapshot_dir(options.sources)?;
    snapshot.retain(|source| source.path != rpmx_components::INVENTORY_FILE);
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
            let mut text = serde_json::to_string_pretty(&lock.redacted())
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
            let mut text = serde_json::to_string_pretty(&lock.redacted())
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
            native: toolchain.native,
            cflags: toolchain.cflags.clone(),
            cxxflags: toolchain.cxxflags.clone(),
            ldflags: toolchain.ldflags.clone(),
        }),
        _ => None,
    };

    let mut packages = Vec::with_capacity(options.plan.cells.len());
    for cell in &options.plan.cells {
        let build = srpm::LockBuild {
            packages: cell.packages.clone(),
            sysroot: cell.sysroot.clone(),
            tests: cell
                .tests
                .iter()
                .flat_map(|test| test.packages.iter().cloned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
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
                replaced: options.replaced,
            },
        )?;
        let lock_digest = srpm::sha512_hex(lock.as_bytes());
        let manifest = srpm::manifest_json(
            &recipe_digest,
            &lock_digest,
            &inventory_digest,
            options.source_date_epoch,
            &snapshot,
            &steps,
            lock_root,
        )?;

        let mut files = vec![
            SourcePackageFile {
                path: "package.yaml".to_string(),
                data: options.recipe_bytes.to_vec(),
                mode: options.recipe_mode,
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
            SourcePackageFile {
                path: rpmx_components::INVENTORY_FILE.to_string(),
                data: inventory_text.as_bytes().to_vec(),
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
                    mode: include.mode,
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

/// Combine target plans into one SRPM while retaining each exact root lock.
pub fn merge_target_packages(mut variants: Vec<SourcePackageSpec>) -> Result<SourcePackageSpec> {
    let mut package = variants
        .pop()
        .ok_or_else(|| Error::new(codes::SRPM_006, "no SRPM targets"))?;
    let mut files = BTreeMap::<String, SourcePackageFile>::new();
    let mut targets = BTreeMap::new();
    let mut steps = BTreeMap::<String, String>::new();
    let mut manifest = None;
    for mut variant in variants
        .into_iter()
        .chain(std::iter::once(SourcePackageSpec {
            files: std::mem::take(&mut package.files),
            ..package.clone()
        }))
    {
        if variant.name != package.name
            || variant.version != package.version
            || variant.release != package.release
            || variant.source_date_epoch != package.source_date_epoch
        {
            return Err(Error::new(
                codes::SRPM_006,
                "SRPM targets have different package identities",
            ));
        }
        let arch = rpmx_base::arch::Arch::parse(&variant.arch)?.to_string();
        let lock_file = variant
            .files
            .iter_mut()
            .find(|file| file.path == "package.lock.json")
            .ok_or_else(|| Error::new(codes::SRPM_006, "target has no package lock"))?;
        let mut lock: srpm::PackageLock = serde_json::from_slice(&lock_file.data)
            .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?;
        if let Some(root) = &mut lock.root_lock {
            root.path = format!("root-{arch}.lock.json");
        }
        if let Some(cross) = &mut lock.cross {
            cross.sysroot.path = format!("sysroot-{arch}.lock.json");
        }
        lock_file.path = format!("package-{arch}.lock.json");
        lock_file.data = serde_json::to_vec_pretty(&lock)
            .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?;
        let reference = srpm::LockRoot {
            path: lock_file.path.clone(),
            digest: srpm::sha512_hex(&lock_file.data),
        };
        if targets.insert(arch.clone(), reference).is_some() {
            return Err(Error::new(codes::SRPM_006, "duplicate SRPM target"));
        }
        for mut file in variant.files {
            match file.path.as_str() {
                "root.lock.json" => file.path = format!("root-{arch}.lock.json"),
                "sysroot.lock.json" => file.path = format!("sysroot-{arch}.lock.json"),
                "RPMX-MANIFEST.json" => {
                    manifest = Some(
                        serde_json::from_slice::<serde_json::Value>(&file.data)
                            .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?,
                    );
                    continue;
                }
                "RPMX-STEPS.json" => {
                    let target_steps: BTreeMap<String, String> = serde_json::from_slice(&file.data)
                        .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?;
                    for (name, text) in target_steps {
                        if steps.get(&name).is_some_and(|previous| previous != &text) {
                            return Err(Error::new(
                                codes::SRPM_006,
                                "target pipeline definitions differ",
                            ));
                        }
                        steps.insert(name, text);
                    }
                    continue;
                }
                _ => {}
            }
            if let Some(previous) = files.get(&file.path) {
                if previous.data != file.data || previous.mode != file.mode {
                    return Err(Error::new(
                        codes::SRPM_006,
                        format!("target source files differ: {}", file.path),
                    ));
                }
            } else {
                files.insert(file.path.clone(), file);
            }
        }
    }
    let index = srpm::TargetIndex {
        schema: 2,
        source_date_epoch: package.source_date_epoch,
        targets,
    };
    for (path, data) in [
        ("package.lock.json", serde_json::to_vec_pretty(&index)),
        ("RPMX-STEPS.json", serde_json::to_vec_pretty(&steps)),
    ] {
        files.insert(
            path.into(),
            SourcePackageFile {
                path: path.into(),
                mode: 0o644,
                data: data.map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?,
            },
        );
    }
    let mut manifest = manifest.ok_or_else(|| Error::new(codes::SRPM_006, "no source manifest"))?;
    manifest["schema"] = 2.into();
    manifest
        .as_object_mut()
        .expect("source manifest object")
        .remove("root_lock");
    manifest["lock"]["digest"] = srpm::sha512_hex(&files["package.lock.json"].data).into();
    manifest["sources"] = serde_json::json!(files
        .values()
        .map(|file| serde_json::json!({
            "path": file.path, "digest": srpm::sha512_hex(&file.data), "size": file.data.len()
        }))
        .collect::<Vec<_>>());
    files.insert(
        "RPMX-MANIFEST.json".into(),
        SourcePackageFile {
            path: "RPMX-MANIFEST.json".into(),
            mode: 0o644,
            data: serde_json::to_vec_pretty(&manifest)
                .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?,
        },
    );
    package.arch = "noarch".into();
    package.files = files.into_values().collect();
    Ok(package)
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
        for test in cell.tests.iter().chain(&cell.build_tests) {
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
