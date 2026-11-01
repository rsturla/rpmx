use std::collections::BTreeSet;
use std::path::Path;

use crate::elf;
use crate::package::{self, OutputSpec};
use crate::package_spec::{BinaryPackageDirectory, BinaryPackageFile, BinaryPackageSpec};
use rpmx_base::error::{codes, Error, Result};
use rpmx_language::model::Recipe;
use rpmx_language::plan::PlannedCell;

pub struct BinaryPackageOptions<'a> {
    pub recipe: &'a Recipe,
    pub cell: &'a PlannedCell,
    pub stage: &'a Path,
    pub arch: &'a str,
    pub source_date_epoch: u32,
    pub debuginfo: bool,
}

pub fn prepare_binary_packages(
    options: BinaryPackageOptions<'_>,
) -> Result<Vec<BinaryPackageSpec>> {
    let staged = package::scan_stage(options.stage)?;
    let mut outputs = options.cell.outputs.clone();
    let debug_name = format!("{}-debuginfo", options.recipe.package.name);
    if options.debuginfo {
        if !staged
            .iter()
            .any(|file| file.path.starts_with("/usr/lib/debug/"))
        {
            return Err(Error::new(
                codes::OUTPUT_012,
                "debuginfo requested but the stage contains no /usr/lib/debug files; \
                 add `uses: strip` and pass --debuginfo",
            ));
        }
        outputs.push(rpmx_language::plan::PlannedOutput {
            name: debug_name.clone(),
            summary: Some(format!(
                "Debug information for {}",
                options.recipe.package.name
            )),
            description: None,
            arch: Some(options.arch.to_string()),
            metapackage: false,
            files: vec!["/usr/lib/debug/**".to_string()],
            exclude: Vec::new(),
            license: Vec::new(),
            doc: Vec::new(),
            config: Vec::new(),
            noreplace: Vec::new(),
            directories: Vec::new(),
            attributes: Vec::new(),
            dependencies: rpmx_language::model::DependencySet {
                requires: vec![format!(
                    "{} = {}-{}",
                    options.cell.package, options.cell.version, options.cell.release
                )],
                provides: Vec::new(),
                ..Default::default()
            },
            scriptlets: Default::default(),
        });
    }
    let specs = outputs
        .iter()
        .map(|output| OutputSpec {
            name: output.name.clone(),
            include: output.files.clone(),
            exclude: output.exclude.clone(),
            metapackage: output.metapackage,
        })
        .collect::<Vec<_>>();
    let split = package::split(&staged, &specs)?;

    let mut packages = Vec::with_capacity(split.len());
    for output_files in split {
        let planned = outputs
            .iter()
            .find(|output| output.name == output_files.name)
            .expect("split output came from the plan");
        let arch = match planned.arch.as_deref() {
            None | Some("any") => options.arch.to_string(),
            Some(value) => value.to_string(),
        };
        let license = package::matcher(&planned.license)?;
        let doc = package::matcher(&planned.doc)?;
        let config = package::matcher(&planned.config)?;
        let noreplace = package::matcher(&planned.noreplace)?;
        let mut files: Vec<BinaryPackageFile> = output_files
            .files
            .into_iter()
            .map(|file| BinaryPackageFile {
                config: config.as_ref().is_some_and(|set| set.is_match(&file.path))
                    || noreplace
                        .as_ref()
                        .is_some_and(|set| set.is_match(&file.path)),
                noreplace: noreplace
                    .as_ref()
                    .is_some_and(|set| set.is_match(&file.path)),
                doc: doc.as_ref().is_some_and(|set| set.is_match(&file.path)),
                license: license.as_ref().is_some_and(|set| set.is_match(&file.path)),
                path: file.path,
                data: file.data,
                mode: file.mode,
                target: file.target,
                user: None,
                group: None,
                caps: None,
            })
            .collect();
        let directories = apply_file_metadata(planned, &mut files)?;

        if planned.name == debug_name {
            packages.push(BinaryPackageSpec {
                name: planned.name.clone(),
                version: options.cell.version.clone(),
                release: options.cell.release.clone(),
                epoch: options.recipe.package.epoch,
                arch: options.arch.to_string(),
                summary: planned
                    .summary
                    .clone()
                    .unwrap_or_else(|| options.recipe.package.summary.clone()),
                license: options.recipe.package.license.clone(),
                description: planned
                    .summary
                    .clone()
                    .unwrap_or_else(|| options.recipe.package.summary.clone()),
                url: None,
                source_rpm: format!(
                    "{}-{}-{}.src.rpm",
                    options.cell.package, options.cell.version, options.cell.release
                ),
                source_date_epoch: options.source_date_epoch,
                files,
                directories,
                dependencies: planned.dependencies.clone(),
                scriptlets: planned.scriptlets.clone(),
            });
            continue;
        }

        let mut provides = planned.dependencies.provides.clone();
        let mut requires = planned.dependencies.requires.clone();
        let mut sonames: BTreeSet<String> = BTreeSet::new();
        for file in &files {
            if file.target.is_some() {
                continue;
            }
            if let Some(info) = elf::inspect(&file.data) {
                if let Some(soname) = &info.soname {
                    let dependency = elf::soname_dependency(soname, info.is_64);
                    sonames.insert(dependency.clone());
                    provides.push(dependency);
                    for version in &info.version_defs {
                        if version == soname {
                            continue;
                        }
                        provides.push(elf::versioned_dependency(soname, version, info.is_64));
                    }
                }
                for needed in &info.needed {
                    requires.push(elf::soname_dependency(needed, info.is_64));
                }
                for (library, version) in &info.version_needs {
                    requires.push(elf::versioned_dependency(library, version, info.is_64));
                }
            }
            if let Some(provide) = elf::pkgconfig_provide(&file.path, &file.data) {
                provides.push(provide);
            }
        }
        requires.retain(|dependency| !sonames.contains(dependency));
        let provides = sorted_unique(provides);
        let requires = sorted_unique(requires);

        packages.push(BinaryPackageSpec {
            name: planned.name.clone(),
            version: options.cell.version.clone(),
            release: options.cell.release.clone(),
            epoch: options.recipe.package.epoch,
            arch,
            summary: planned
                .summary
                .clone()
                .unwrap_or_else(|| options.recipe.package.summary.clone()),
            license: options.recipe.package.license.clone(),
            description: planned
                .description
                .clone()
                .or_else(|| options.recipe.package.description.clone())
                .unwrap_or_else(|| options.recipe.package.summary.clone()),
            url: options.recipe.package.upstream.clone(),
            source_rpm: format!(
                "{}-{}-{}.src.rpm",
                options.cell.package, options.cell.version, options.cell.release
            ),
            source_date_epoch: options.source_date_epoch,
            files,
            directories,
            dependencies: rpmx_language::model::DependencySet {
                requires,
                recommends: planned.dependencies.recommends.clone(),
                suggests: planned.dependencies.suggests.clone(),
                supplements: planned.dependencies.supplements.clone(),
                enhances: planned.dependencies.enhances.clone(),
                provides,
                obsoletes: planned.dependencies.obsoletes.clone(),
                conflicts: planned.dependencies.conflicts.clone(),
            },
            scriptlets: planned.scriptlets.clone(),
        });
    }
    Ok(packages)
}

fn apply_file_metadata(
    planned: &rpmx_language::plan::PlannedOutput,
    files: &mut [BinaryPackageFile],
) -> Result<Vec<BinaryPackageDirectory>> {
    let attributes = planned
        .attributes
        .iter()
        .map(|attribute| (attribute.path.as_str(), attribute))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    for file in files.iter_mut() {
        let Some(attribute) = attributes.get(file.path.as_str()) else {
            continue;
        };
        seen.insert(file.path.as_str());
        if file.target.is_some() {
            if attribute.mode.is_some() || attribute.caps.is_some() {
                return Err(Error::new(
                    codes::OUTPUT_016,
                    format!(
                        "attributes for symlink '{}' cannot set mode or capabilities",
                        file.path
                    ),
                ));
            }
        } else {
            if let Some(mode) = &attribute.mode {
                file.mode = parse_octal_mode(mode)?;
            }
            if let Some(caps) = &attribute.caps {
                file.caps = Some(caps.clone());
            }
        }
        file.user = attribute.user.clone();
        file.group = attribute.group.clone();
    }
    if let Some(missing) = planned
        .attributes
        .iter()
        .find(|attribute| !seen.contains(attribute.path.as_str()))
    {
        return Err(Error::new(
            codes::OUTPUT_018,
            format!(
                "output '{}' sets attributes for '{}' but no packaged file matches that path",
                planned.name, missing.path
            ),
        ));
    }

    let file_paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    planned
        .directories
        .iter()
        .map(|directory| {
            if file_paths.contains(directory.path.as_str()) {
                return Err(Error::new(
                    codes::OUTPUT_017,
                    format!(
                        "output '{}' declares '{}' as both a directory and a file",
                        planned.name, directory.path
                    ),
                ));
            }
            Ok(BinaryPackageDirectory {
                path: directory.path.clone(),
                mode: match &directory.mode {
                    Some(mode) => parse_octal_mode(mode)?,
                    None => 0o755,
                },
                user: directory.user.clone(),
                group: directory.group.clone(),
            })
        })
        .collect()
}

fn parse_octal_mode(mode: &str) -> Result<u32> {
    u32::from_str_radix(mode, 8).map_err(|_| {
        Error::new(
            codes::OUTPUT_016,
            format!("mode '{mode}' is not an octal string"),
        )
    })
}

fn sorted_unique(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
