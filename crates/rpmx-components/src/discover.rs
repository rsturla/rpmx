use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rpmx_base::error::{codes, Error, Result};
use rpmx_language::plan::PlannedCell;

use crate::catalog;
use crate::model::{CatalogerRef, Component, Inventory};
use crate::BUNDLED_DIR;

pub(crate) struct DiscoveryContext<'a> {
    root: &'a Path,
    files: &'a [PathBuf],
    dirs: &'a [PathBuf],
    outputs: &'a BTreeSet<String>,
    auto_outputs: &'a BTreeSet<String>,
    package_names: &'a BTreeSet<String>,
}

impl DiscoveryContext<'_> {
    pub(crate) fn root(&self) -> &Path {
        self.root
    }

    pub(crate) fn files(&self) -> &[PathBuf] {
        self.files
    }

    pub(crate) fn dirs(&self) -> &[PathBuf] {
        self.dirs
    }

    pub(crate) fn outputs(&self) -> &BTreeSet<String> {
        self.outputs
    }

    pub(crate) fn auto_outputs(&self) -> &BTreeSet<String> {
        self.auto_outputs
    }

    pub(crate) fn package_names(&self) -> &BTreeSet<String> {
        self.package_names
    }

    pub(crate) fn relative(&self, path: &Path) -> String {
        path.strip_prefix(self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

pub(crate) trait Cataloger {
    fn name(&self) -> &'static str;
    fn version(&self) -> u32 {
        1
    }
    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>>;
}

fn default_catalogers() -> Vec<Box<dyn Cataloger>> {
    vec![
        Box::new(catalog::go::GoModules),
        Box::new(catalog::cargo::CargoLock),
        Box::new(catalog::node::NodeLock),
        Box::new(catalog::node::NodeInstalled),
        Box::new(catalog::yarn::YarnLock),
        Box::new(catalog::pnpm::PnpmLock),
        Box::new(catalog::python::PythonDist),
        Box::new(catalog::ruby::RubyGems),
        Box::new(catalog::php::Composer),
        Box::new(catalog::manual::ManualBuild),
    ]
}

pub fn discover(root: &Path, cells: &[&PlannedCell]) -> Result<Inventory> {
    let mut inventory = Inventory::from_cells(cells);
    let outputs = cells
        .iter()
        .flat_map(|cell| cell.outputs.iter().map(|output| output.name.clone()))
        .collect::<BTreeSet<_>>();
    let auto_outputs = primary_outputs(cells);
    let package_names = cells
        .iter()
        .map(|cell| cell.package.clone())
        .collect::<BTreeSet<_>>();
    let (files, dirs) = walk(root)?;
    let context = DiscoveryContext {
        root,
        files: &files,
        dirs: &dirs,
        outputs: &outputs,
        auto_outputs: &auto_outputs,
        package_names: &package_names,
    };

    for cataloger in default_catalogers() {
        let components = cataloger.detect(&context)?;
        if components.is_empty() {
            continue;
        }
        inventory.components.extend(components);
        inventory.catalogers.push(CatalogerRef {
            name: cataloger.name().to_string(),
            version: cataloger.version(),
        });
    }

    inventory.normalize()?;
    Ok(inventory)
}

/// Create the declaration directories before a sandbox starts. A typo in a
/// step therefore fails when it writes, rather than creating an orphan tree.
pub fn prepare_output_dirs(root: &Path, cells: &[&PlannedCell]) -> Result<()> {
    let rpmx = root.join(".rpmx");
    create_declaration_dir(&rpmx)?;
    let base = root.join(BUNDLED_DIR);
    create_declaration_dir(&base)?;
    for cell in cells {
        for output in &cell.outputs {
            if output.name.is_empty()
                || output.name == "."
                || output.name == ".."
                || output.name.contains('/')
                || output.name.contains('\\')
            {
                return Err(Error::new(
                    codes::BUNDLED_003,
                    format!(
                        "output '{}' cannot be used as a bundled declaration directory",
                        output.name
                    ),
                ));
            }
            create_declaration_dir(&base.join(&output.name))?;
        }
    }
    Ok(())
}

fn create_declaration_dir(path: &Path) -> Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::new(
                codes::BUNDLED_003,
                format!("{} must be a real directory", path.display()),
            ));
        }
        return Ok(());
    }
    std::fs::create_dir_all(path).map_err(|error| {
        Error::new(
            codes::BUNDLED_003,
            format!("cannot create {}: {error}", path.display()),
        )
    })
}

fn primary_outputs(cells: &[&PlannedCell]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for cell in cells {
        if let Some(output) = cell
            .outputs
            .iter()
            .find(|output| output.name == cell.package && !output.metapackage)
        {
            names.insert(output.name.clone());
            continue;
        }
        if let Some(output) = cell.outputs.iter().find(|output| !output.metapackage) {
            names.insert(output.name.clone());
        }
    }
    names
}

fn walk(root: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    walk_dir(root, &mut files, &mut dirs)?;
    files.sort();
    dirs.sort();
    Ok((files, dirs))
}

fn walk_dir(directory: &Path, files: &mut Vec<PathBuf>, dirs: &mut Vec<PathBuf>) -> Result<()> {
    for entry in rpmx_base::io::sorted_entries(directory, codes::BUNDLED_003)? {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".git" | "target" | ".rpmx") {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            Error::new(
                codes::BUNDLED_003,
                format!("cannot stat {}: {error}", path.display()),
            )
        })?;
        if file_type.is_dir() {
            dirs.push(path.clone());
            if name == "node_modules" {
                continue;
            }
            walk_dir(&path, files, dirs)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

pub(crate) fn read_text(path: &Path) -> Result<String> {
    const MAX: u64 = 8 * 1024 * 1024;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        Error::new(
            codes::BUNDLED_001,
            format!("cannot stat {}: {error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::new(
            codes::BUNDLED_001,
            format!("{} is not a regular file", path.display()),
        ));
    }
    if metadata.len() > MAX {
        return Err(Error::new(
            codes::BUNDLED_001,
            format!("{} exceeds the {MAX}-byte manifest limit", path.display()),
        ));
    }
    std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::BUNDLED_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })
}
