//! Binary RPM header metadata used by test-root selection.

use std::collections::BTreeSet;
use std::path::Path;

use rpmx_base::error::{codes, Error, Result};

/// Runtime dependency information from a binary RPM header.
#[derive(Debug, Clone)]
pub struct PackageMetadata {
    pub name: String,
    pub epoch: u32,
    pub version: String,
    pub release: String,
    pub provides: Vec<rpm::Dependency>,
    pub requires: Vec<rpm::Dependency>,
    pub files: BTreeSet<String>,
}

impl PackageMetadata {
    /// The package's own epoch-version-release, in the form used by provides.
    pub fn evr(&self) -> String {
        if self.epoch == 0 {
            format!("{}-{}", self.version, self.release)
        } else {
            format!("{}:{}-{}", self.epoch, self.version, self.release)
        }
    }

    /// Capabilities this package provides, including its own name and files.
    pub fn provided_capabilities(&self) -> BTreeSet<String> {
        let mut capabilities = self
            .provides
            .iter()
            .map(|dependency| dependency.name.clone())
            .collect::<BTreeSet<_>>();
        capabilities.insert(self.name.clone());
        capabilities.extend(self.files.iter().cloned());
        capabilities
    }

    /// Names of all runtime requirements.
    pub fn required_names(&self) -> BTreeSet<String> {
        self.requires
            .iter()
            .map(|dependency| dependency.name.clone())
            .collect()
    }
}

/// Read the name, EVR, capabilities and file list from a binary RPM header.
pub fn package_metadata(path: &Path) -> Result<PackageMetadata> {
    let package = rpm::Package::open(path).map_err(|error| {
        Error::new(
            codes::EXTRACT_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let metadata = &package.metadata;
    let name = metadata
        .get_name()
        .map_err(|error| {
            Error::new(
                codes::EXTRACT_001,
                format!(
                    "{} has no package name in its header: {error}",
                    path.display()
                ),
            )
        })?
        .to_string();
    let epoch = metadata.get_epoch().map_err(|error| {
        Error::new(
            codes::EXTRACT_001,
            format!("cannot read epoch from {}: {error}", path.display()),
        )
    })?;
    let version = metadata
        .get_version()
        .map_err(|error| {
            Error::new(
                codes::EXTRACT_001,
                format!("cannot read version from {}: {error}", path.display()),
            )
        })?
        .to_string();
    let release = metadata
        .get_release()
        .map_err(|error| {
            Error::new(
                codes::EXTRACT_001,
                format!("cannot read release from {}: {error}", path.display()),
            )
        })?
        .to_string();
    let provides = dependencies(metadata.get_provides().map_err(|error| {
        Error::new(
            codes::EXTRACT_001,
            format!("cannot read provides from {}: {error}", path.display()),
        )
    })?);
    let requires = dependencies(metadata.get_requires().map_err(|error| {
        Error::new(
            codes::EXTRACT_001,
            format!("cannot read requires from {}: {error}", path.display()),
        )
    })?);
    let files = metadata
        .get_file_paths()
        .map_err(|error| {
            Error::new(
                codes::EXTRACT_001,
                format!("cannot read files from {}: {error}", path.display()),
            )
        })?
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    Ok(PackageMetadata {
        name,
        epoch,
        version,
        release,
        provides,
        requires,
        files,
    })
}

fn dependencies(mut dependencies: Vec<rpm::Dependency>) -> Vec<rpm::Dependency> {
    dependencies.sort_by(|left, right| {
        (&left.name, left.flags.bits(), &left.version).cmp(&(
            &right.name,
            right.flags.bits(),
            &right.version,
        ))
    });
    dependencies.dedup_by(|left, right| {
        left.name == right.name && left.flags == right.flags && left.version == right.version
    });
    dependencies
}
