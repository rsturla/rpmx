use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::profile::{normalize_arch, Repository, ResolveOptions};
use rpmx_base::error::{codes, Error, Result};
use rpmx_base::hash::sha512_hex;

pub const DOCUMENT: &str = "rpmx-root-lock";
pub const VERSION: u32 = 1;

/// A resolved build root: repositories, an exact package closure and the
/// builder that produced it.
///
/// The `repositories` and `packages` sections use the libpkgmanifest
/// `rpm-package-manifest` vocabulary. The document header and integrity fields
/// are rpmx's own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootLock {
    pub document: String,
    pub version: u32,
    pub profile: String,
    pub archs: Vec<String>,
    pub releasever: String,
    /// Distribution tag recorded from the profile. It is provenance, not
    /// closure content, so it is deliberately outside `closure_digest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist_tag: Option<String>,
    #[serde(default)]
    pub options: ResolveOptions,
    /// Install prefix recorded from the profile. It is provenance, not closure
    /// content, so it is deliberately outside `closure_digest`.
    #[serde(default = "crate::profile::default_prefix")]
    pub prefix: String,
    /// Library directory recorded from the profile.
    #[serde(default = "crate::profile::default_libdir")]
    pub libdir: String,
    #[serde(default)]
    pub debuginfo: bool,
    #[serde(default = "crate::profile::default_sbom")]
    pub sbom: bool,
    pub repositories: Vec<Repository>,
    pub packages: BTreeMap<String, Vec<LockedPackage>>,
    pub closure_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builder: Option<LockedBuilder>,
}

impl RootLock {
    /// The lock must describe the architecture being built.
    pub fn require_target(&self, arch: &str) -> Result<()> {
        let wanted = normalize_arch(arch)?;
        if self
            .archs
            .iter()
            .any(|candidate| normalize_arch(candidate).ok() == Some(wanted))
        {
            Ok(())
        } else {
            Err(Error::new(
                codes::ROOT_016,
                format!(
                    "root lock is for {}, but --arch is {wanted}",
                    self.archs.join(", ")
                ),
            ))
        }
    }

    /// The lock must have an assembled root digest before it can be used.
    pub fn require_digest(&self) -> Result<&str> {
        let digest = self.root_digest.as_deref().ok_or_else(|| {
            Error::new(
                codes::ROOT_016,
                "the root lock has no root_digest; run `rpmx root assemble` first",
            )
        })?;
        let valid = digest.strip_prefix("sha512:").is_some_and(|hex| {
            hex.len() == 128 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        if !valid {
            return Err(Error::new(
                codes::ROOT_016,
                format!("root lock has an invalid root_digest '{digest}'"),
            ));
        }
        Ok(digest)
    }
}

/// A lock plus the verified assembled root it describes.
pub struct RootSelection {
    pub lock: RootLock,
    pub path: std::path::PathBuf,
    pub digest: String,
}

impl RootSelection {
    pub fn verify(lock: &RootLock, root: &Path, arch: &str) -> Result<Self> {
        lock.require_target(arch)?;
        let expected = lock.require_digest()?;
        let path = std::fs::canonicalize(root).map_err(|error| {
            Error::new(
                codes::ROOT_017,
                format!("cannot resolve {}: {error}", root.display()),
            )
        })?;
        let digest = rpmx_store::tree::digest_root(&path)?;
        if digest != expected {
            return Err(Error::new(
                codes::ROOT_017,
                format!(
                    "root digest mismatch: lock records {expected}, {} is {digest}",
                    path.display()
                ),
            ));
        }
        Ok(Self {
            lock: lock.clone(),
            path,
            digest,
        })
    }

    pub fn load(lock_path: &Path, root_path: &Path, arch: &str) -> Result<Self> {
        let lock = load_lock(lock_path)?;
        Self::verify(&lock, root_path, arch)
    }
}

/// The identity of one installed package, independent of its repository.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageKey {
    pub name: String,
    pub epoch: String,
    pub version: String,
    pub release: String,
    pub arch: String,
}

impl PackageKey {
    pub fn new(name: &str, epoch: &str, version: &str, release: &str, arch: &str) -> Self {
        Self {
            name: name.to_string(),
            epoch: if epoch.is_empty() || epoch == "(none)" {
                "0"
            } else {
                epoch
            }
            .to_string(),
            version: version.to_string(),
            release: release.to_string(),
            arch: arch.to_string(),
        }
    }
}

impl std::fmt::Display for PackageKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}|{}|{}|{}|{}",
            self.name, self.epoch, self.version, self.release, self.arch
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedBuilder {
    pub image: String,
    /// Manifest digest for the lock's target architecture, not the multi-arch
    /// index digest.
    pub image_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub name: String,
    pub repo_id: String,
    pub evr: String,
    pub checksum: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub srpm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// The reproducible input: repositories plus the exact package closure.
///
/// Volatile fields (`root_digest`, builder image, `location`) are deliberately
/// excluded so two resolutions of the same inputs agree.
pub fn closure_digest(
    releasever: &str,
    archs: &[String],
    repositories: &[Repository],
    packages: &BTreeMap<String, Vec<LockedPackage>>,
) -> String {
    let mut material = String::from("rpmx-root-lock/1\n");
    material.push_str(&format!("releasever:{releasever}\n"));

    let mut sorted_archs = archs.to_vec();
    sorted_archs.sort();
    for arch in sorted_archs {
        material.push_str(&format!("arch:{arch}\n"));
    }

    let mut sorted_repos = repositories.to_vec();
    sorted_repos.sort_by(|left, right| left.id.cmp(&right.id));
    for repository in sorted_repos {
        let (kind, url) = repository.location();
        material.push_str(&format!("repo:{}:{kind}:{url}\n", repository.id));
    }

    for (arch, entries) in packages {
        let mut sorted = entries.to_vec();
        sorted.sort_by(|left, right| {
            (&left.name, &left.evr, &left.checksum).cmp(&(&right.name, &right.evr, &right.checksum))
        });
        for package in sorted {
            material.push_str(&format!(
                "pkg:{arch}:{}:{}:{}:{}:{}:{}:{}\n",
                package.name,
                package.evr,
                package.repo_id,
                package.checksum,
                package.size,
                package.srpm.as_deref().unwrap_or(""),
                package.module.as_deref().unwrap_or(""),
            ));
        }
    }

    sha512_hex(material.as_bytes())
}

/// Load a lock and check that it is a document this rpmx understands.
pub fn load_lock(path: &Path) -> Result<RootLock> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot read root lock {}: {error}", path.display()),
        )
    })?;
    let lock: RootLock = serde_yaml_ng::from_str(&text)
        .map_err(|error| Error::new(codes::ROOT_006, format!("invalid root lock: {error}")))?;
    if lock.document != DOCUMENT {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "root lock document must be '{DOCUMENT}', found '{}'",
                lock.document
            ),
        ));
    }
    if lock.version != VERSION {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "unsupported root lock version {}; this rpmx understands {VERSION}",
                lock.version
            ),
        ));
    }
    for arch in &lock.archs {
        normalize_arch(arch)?;
    }
    let expected = closure_digest(
        &lock.releasever,
        &lock.archs,
        &lock.repositories,
        &lock.packages,
    );
    if lock.closure_digest != expected {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "root lock closure digest mismatch: recorded {}, recomputed {expected}",
                lock.closure_digest
            ),
        ));
    }
    Ok(lock)
}

pub fn write_lock(path: &Path, lock: &RootLock) -> Result<()> {
    let text = serde_yaml_ng::to_string(lock)
        .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
    std::fs::write(path, text).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}
