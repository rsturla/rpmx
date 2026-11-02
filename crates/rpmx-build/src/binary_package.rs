use std::collections::BTreeSet;
use std::path::Path;

use crate::generators;
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
    let target = rpmx_base::arch::Arch::parse(options.arch)?;
    for file in &staged {
        if let Some(info) = crate::elf::inspect(&file.data) {
            crate::elf::require_machine(&file.path, &info, target)?;
        }
    }
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
            generators: Default::default(),
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
    let sonames = generators::sonames(&staged);

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

        let (generated_provides, generated_requires) =
            generators::generate(&files, options.arch, &sonames, &planned.generators);
        let mut provides = planned.dependencies.provides.clone();
        provides.extend(generated_provides);
        let mut requires = planned.dependencies.requires.clone();
        requires.extend(generated_requires);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(text: &str, arch: &str) -> (Recipe, PlannedCell) {
        let recipe = rpmx_language::parse_recipe(text).expect("recipe");
        let plan = rpmx_language::plan_with_options(
            &recipe,
            rpmx_language::PlanOptions {
                arch,
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan");
        let cell = plan.cells.into_iter().next().expect("one cell");
        (recipe, cell)
    }

    fn fixture() -> &'static str {
        r#"
schema: 1
package:
  name: demo
  version: "1"
  release: 1
  summary: demo
  license: MIT
outputs:
  - name: demo
    files:
      include: ["/usr/bin/**"]
"#
    }

    #[test]
    fn a_staged_binary_for_another_architecture_is_rejected() {
        let host = rpmx_base::arch::Arch::current().expect("host");
        let other = match host {
            rpmx_base::arch::Arch::X86_64 => rpmx_base::arch::Arch::Aarch64,
            _ => rpmx_base::arch::Arch::X86_64,
        };
        let stage = tempfile::tempdir().expect("stage");
        std::fs::create_dir_all(stage.path().join("usr/bin")).expect("dir");
        std::fs::copy(
            std::env::current_exe().expect("test binary"),
            stage.path().join("usr/bin/demo"),
        )
        .expect("copy");

        let (recipe, cell) = plan(fixture(), other.as_str());
        let error = prepare_binary_packages(BinaryPackageOptions {
            recipe: &recipe,
            cell: &cell,
            stage: stage.path(),
            arch: other.as_str(),
            source_date_epoch: 1,
            debuginfo: false,
        })
        .expect_err("a host binary must not be packaged as a foreign one");
        assert_eq!(error.code, "RPMX-BUILD-008");
        assert!(error.message.contains("machine"), "{}", error.message);

        // The same payload is fine for the architecture it was built for.
        let (recipe, cell) = plan(fixture(), host.as_str());
        prepare_binary_packages(BinaryPackageOptions {
            recipe: &recipe,
            cell: &cell,
            stage: stage.path(),
            arch: host.as_str(),
            source_date_epoch: 1,
            debuginfo: false,
        })
        .expect("host payload");
    }
}
