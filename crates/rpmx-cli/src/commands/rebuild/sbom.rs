use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::package_spec::{BinaryPackageFile, BinaryPackageSpec};
use rpmx_core::plan::PlannedCell;
use rpmx_core::sbom::{BinaryRef, BuildSbom, Generator, PackageSbom, SourceRef};
use rpmx_core::srpm::PackageLock;
use rpmx_core::{Error, Recipe};

pub(super) struct BuildSbomInput<'a> {
    pub(super) recipe: &'a Recipe,
    pub(super) cell: &'a PlannedCell,
    pub(super) arch: &'a str,
    pub(super) source_date_epoch: u32,
    pub(super) binaries: &'a [BinaryRef<'a>],
    pub(super) root_lock: Option<&'a rpmx_core::root::RootLock>,
    pub(super) signing_key: Option<&'a str>,
    pub(super) path: &'a Path,
}

/// Everything the SBOM passes need: source identity and the SRPM name.
pub(super) struct SbomContext {
    recipe_digest: String,
    sources: Vec<(String, String)>,
    source_rpm: String,
}

impl SbomContext {
    pub(super) fn prepare(package_lock: &PackageLock, work: &Path, cell: &PlannedCell) -> Self {
        let mut recipe_digest = package_lock.recipe_digest.clone();
        if recipe_digest.is_empty() {
            if let Ok(bytes) = std::fs::read(work.join("package.yaml")) {
                recipe_digest = rpmx_core::srpm::sha512_hex(&bytes);
            }
        }
        Self {
            recipe_digest,
            sources: package_lock
                .sources
                .iter()
                // The source root lock records the root used to assemble the
                // sources. It is build metadata, not an input to the package,
                // and rebuilds of the same recipe from different roots would
                // otherwise make every binary carry a different SBOM.
                .filter(|source| source.path != "source-root.lock.json")
                .map(|source| (source.path.clone(), source.digest.clone()))
                .collect(),
            source_rpm: format!("{}-{}-{}.src.rpm", cell.package, cell.version, cell.release),
        }
    }

    fn refs(&self) -> Vec<SourceRef<'_>> {
        self.sources
            .iter()
            .map(|(path, digest)| SourceRef { path, digest })
            .collect()
    }

    /// Attach the per-package SPDX document to the package payload.
    pub(super) fn attach_package(
        &self,
        package: &mut BinaryPackageSpec,
        recipe: &Recipe,
    ) -> Result<(), Error> {
        let refs = self.refs();
        let text = rpmx_core::sbom::native_spdx().package(&PackageSbom {
            name: &package.name,
            version: &package.version,
            release: &package.release,
            arch: &package.arch,
            license: &package.license,
            source_rpm: &self.source_rpm,
            recipe_digest: &self.recipe_digest,
            source_date_epoch: package.source_date_epoch,
            sources: &refs,
            cpe: recipe.package.cpe.as_ref(),
        })?;
        package.files.push(BinaryPackageFile {
            path: format!(
                "{}/{}-{}-{}.spdx.json",
                rpmx_core::sbom::SBOM_DIR,
                package.name,
                package.version,
                package.release
            ),
            data: text.into_bytes(),
            mode: 0o644,
            target: None,
            user: None,
            group: None,
            caps: None,
            config: false,
            noreplace: false,
            doc: false,
            license: false,
        });
        Ok(())
    }

    /// Write the build-level SPDX document next to the binaries.
    pub(super) fn emit_build(&self, input: BuildSbomInput<'_>) -> Result<(), Error> {
        let refs = self.refs();
        let text = rpmx_core::sbom::native_spdx().build(&BuildSbom {
            name: &input.cell.package,
            version: &input.cell.version,
            release: &input.cell.release,
            arch: input.arch,
            license: &input.recipe.package.license,
            recipe_digest: &self.recipe_digest,
            source_date_epoch: input.source_date_epoch,
            sources: &refs,
            binaries: input.binaries,
            root_lock: input.root_lock,
            signing_key: input.signing_key,
            cpe: input.recipe.package.cpe.as_ref(),
        })?;
        std::fs::write(input.path, text).map_err(|error| {
            Error::new(
                codes::SBOM_001,
                format!("cannot write {}: {error}", input.path.display()),
            )
        })
    }
}
