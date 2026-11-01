use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{codes, Error, Result};
use crate::profile::{normalize_arch, Repository, ResolveOptions};
use crate::root::{closure_digest, LockedPackage, RootLock};

pub const DOCUMENT: &str = "rpm-package-manifest";
pub const VERSION: &str = "0.2.3";

/// The libpkgmanifest `rpm-package-manifest` document.
///
/// rpmx exports this shape at its boundary and imports it, but never uses it as
/// the authoritative lock: it has no place for the closure digest or the
/// builder, and the upstream parser ignores unknown fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpmPackageManifest {
    pub document: String,
    pub version: String,
    pub data: ManifestData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestData {
    pub repositories: Vec<ManifestRepository>,
    pub packages: BTreeMap<String, Vec<ManifestPackage>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestRepository {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseurl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metalink: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirrorlist: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPackage {
    pub name: String,
    pub repo_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub checksum: String,
    pub size: u64,
    pub evr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub srpm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_archs: Option<Vec<String>>,
}

pub fn from_lock(lock: &RootLock) -> RpmPackageManifest {
    let repositories = lock
        .repositories
        .iter()
        .map(|repository| ManifestRepository {
            id: repository.id.clone(),
            baseurl: repository.baseurl.clone(),
            metalink: repository.metalink.clone(),
            mirrorlist: repository.mirrorlist.clone(),
        })
        .collect();

    let mut packages = BTreeMap::new();
    for (arch, entries) in &lock.packages {
        let parent_archs = (arch == "noarch").then(|| lock.archs.clone());
        packages.insert(
            arch.clone(),
            entries
                .iter()
                .map(|package| ManifestPackage {
                    name: package.name.clone(),
                    repo_id: package.repo_id.clone(),
                    location: package.location.clone(),
                    checksum: package.checksum.clone(),
                    size: package.size,
                    evr: package.evr.clone(),
                    srpm: package.srpm.clone(),
                    module: package.module.clone(),
                    parent_archs: parent_archs.clone(),
                })
                .collect(),
        );
    }

    RpmPackageManifest {
        document: DOCUMENT.to_string(),
        version: VERSION.to_string(),
        data: ManifestData {
            repositories,
            packages,
        },
    }
}

pub fn to_lock(
    manifest: &RpmPackageManifest,
    profile: &str,
    arch: Option<&str>,
    releasever: &str,
) -> Result<RootLock> {
    if manifest.document != DOCUMENT {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "manifest document must be '{DOCUMENT}', found '{}'",
                manifest.document
            ),
        ));
    }
    if manifest.version != VERSION {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "unsupported manifest version {}; this rpmx understands {VERSION}",
                manifest.version
            ),
        ));
    }

    let repositories = manifest
        .data
        .repositories
        .iter()
        .map(|repository| Repository {
            id: repository.id.clone(),
            baseurl: repository.baseurl.clone(),
            metalink: repository.metalink.clone(),
            mirrorlist: repository.mirrorlist.clone(),
            gpgkey: None,
        })
        .collect::<Vec<_>>();

    let mut packages = BTreeMap::new();
    for (package_arch, entries) in &manifest.data.packages {
        if package_arch == "src" {
            continue;
        }
        let mut locked = Vec::with_capacity(entries.len());
        for package in entries {
            locked.push(LockedPackage {
                name: package.name.clone(),
                repo_id: package.repo_id.clone(),
                evr: package.evr.clone(),
                checksum: package.checksum.clone(),
                size: package.size,
                srpm: package.srpm.clone(),
                module: package.module.clone(),
                location: package.location.clone(),
            });
        }
        packages.insert(package_arch.clone(), locked);
    }
    if packages.is_empty() {
        return Err(Error::new(
            codes::ROOT_009,
            "manifest contains no binary packages",
        ));
    }

    let archs = match arch {
        Some(arch) => vec![normalize_arch(arch)?.to_string()],
        None => packages
            .keys()
            .filter(|key| *key != "noarch")
            .cloned()
            .collect::<Vec<_>>(),
    };
    if archs.is_empty() {
        return Err(Error::new(
            codes::ROOT_009,
            "cannot infer the target architecture; pass --arch",
        ));
    }
    for package_arch in packages.keys() {
        if package_arch != "noarch" {
            normalize_arch(package_arch)?;
        }
    }

    let options = ResolveOptions::default();
    let closure = closure_digest(releasever, &archs, &repositories, &packages);
    Ok(RootLock {
        document: crate::root::DOCUMENT.to_string(),
        version: crate::root::VERSION,
        profile: profile.to_string(),
        archs,
        releasever: releasever.to_string(),
        dist_tag: None,
        options,
        debuginfo: false,
        sbom: true,
        repositories,
        packages,
        closure_digest: closure,
        root_digest: None,
        builder: None,
    })
}
