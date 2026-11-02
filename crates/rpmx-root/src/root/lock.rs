use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::profile::{normalize_arch, Repository, ResolveOptions};
use rpmx_base::error::{codes, Error, Result};
use rpmx_base::hash::sha512_hex;

pub const DOCUMENT: &str = "rpmx-root-lock";
pub const VERSION: u32 = 1;

/// Version of the runner-owned appliance package set and boot protocol.
///
/// Bump this when `APPLIANCE_PACKAGES` or the QEMU boot/mount contract changes.
/// An old lock is then rejected with a re-resolve hint instead of booting an
/// appliance that no longer matches the runner.
pub const APPLIANCE_SCHEMA: u32 = 1;

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
    /// Runner-owned locked artifacts. This is where a backend that needs more
    /// than the build root declares its assets.
    #[serde(
        default,
        rename = "runner-assets",
        alias = "appliance",
        skip_serializing_if = "Option::is_none"
    )]
    pub runner_assets: Option<RunnerAssetsLock>,
}

/// The runner assets recorded in a root lock.
///
/// Only QEMU has an asset today. A future asset-bearing runner adds a field
/// here and a descriptor in `rpmx-runner`; the build root and closure digest do
/// not change.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerAssetsLock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qemu_appliance: Option<ApplianceLock>,
}

impl RunnerAssetsLock {
    pub fn validate(&self) -> Result<()> {
        if let Some(appliance) = &self.qemu_appliance {
            appliance.validate()?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.qemu_appliance.is_none()
    }
}

impl RootLock {
    pub fn qemu_appliance(&self) -> Option<&ApplianceLock> {
        self.runner_assets
            .as_ref()
            .and_then(|assets| assets.qemu_appliance.as_ref())
    }

    pub fn set_qemu_appliance(&mut self, appliance: Option<ApplianceLock>) {
        self.runner_assets = appliance.map(|appliance| RunnerAssetsLock {
            qemu_appliance: Some(appliance),
        });
    }
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

/// A runner-owned QEMU transport appliance: the kernel, init system, network
/// and SSH tooling needed to boot a VM and enter the build root.
///
/// It is a full RPM closure, like a build root, but it is not part of the build
/// environment. There is no tree digest: the initramfs is generated by a
/// scriptlet and is not a build input. The closure digest and the builder image
/// digest are the identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplianceLock {
    pub schema: u32,
    pub profile: String,
    pub archs: Vec<String>,
    pub releasever: String,
    pub builder: LockedBuilder,
    pub repositories: Vec<Repository>,
    pub packages: BTreeMap<String, Vec<LockedPackage>>,
    pub closure_digest: String,
}

/// A value that is used as one component of a staged RPM filename.
///
/// Package names and EVRs come from a lock, which is untrusted input. Reject
/// path separators and control characters before anything joins them into a
/// path. The build closure and the appliance closure share this rule.
pub(crate) fn safe_staging_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
}

fn sanitize_message(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect()
}

fn validate_architecture_agreement(lock: &RootLock, appliance: &ApplianceLock) -> Result<()> {
    let mut appliance_archs = appliance.archs.clone();
    let mut build_archs = lock.archs.clone();
    appliance_archs.sort();
    build_archs.sort();
    if appliance_archs != build_archs {
        return Err(Error::new(
            codes::ROOT_021,
            "the QEMU appliance architecture does not match the build root architecture",
        ));
    }
    Ok(())
}

fn valid_sha512_digest(value: &str) -> bool {
    value
        .strip_prefix("sha512:")
        .is_some_and(|hex| hex.len() == 128 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// A package checksum is a CAS digest. Full length is enforced by the CAS when
/// the payload is loaded; this check only rejects a value that could be
/// mistaken for a path or that is not a SHA-512 digest at all.
fn valid_package_checksum(value: &str) -> bool {
    value
        .strip_prefix("sha512:")
        .is_some_and(|hex| !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn validate_package_keys(
    packages: &BTreeMap<String, Vec<LockedPackage>>,
    code: rpmx_base::error::Code,
) -> Result<()> {
    for (package_arch, entries) in packages {
        if !safe_staging_component(package_arch) {
            return Err(Error::new(
                code,
                "a package architecture contains a path separator or control character",
            ));
        }
        for package in entries {
            if !safe_staging_component(&package.name)
                || !safe_staging_component(&package.evr)
                || !valid_package_checksum(&package.checksum)
            {
                return Err(Error::new(
                    code,
                    "a package has a name, EVR, or checksum that cannot be staged safely",
                ));
            }
        }
    }
    Ok(())
}

impl ApplianceLock {
    pub fn validate(&self) -> Result<()> {
        if self.schema != APPLIANCE_SCHEMA {
            return Err(Error::new(
                codes::ROOT_021,
                format!(
                    "QEMU appliance schema {} is not supported; this rpmx understands {APPLIANCE_SCHEMA}; re-resolve the root",
                    self.schema
                ),
            ));
        }
        if self.archs.is_empty() || self.archs.len() > 1 {
            return Err(Error::new(
                codes::ROOT_021,
                "the QEMU appliance must declare exactly one architecture",
            ));
        }
        for arch in &self.archs {
            if !safe_staging_component(arch) {
                return Err(Error::new(
                    codes::ROOT_021,
                    "the QEMU appliance architecture contains a control character",
                ));
            }
            normalize_arch(arch).map_err(|error| {
                Error::new(
                    codes::ROOT_021,
                    format!("invalid QEMU appliance architecture: {}", error.message),
                )
            })?;
        }
        if self.releasever.trim().is_empty()
            || self.profile.chars().any(char::is_control)
            || self.builder.image.chars().any(char::is_control)
        {
            return Err(Error::new(
                codes::ROOT_021,
                "the QEMU appliance releasever, profile, or builder image is invalid",
            ));
        }
        if self.builder.image.trim().is_empty() || self.builder.image_digest.trim().is_empty() {
            return Err(Error::new(
                codes::ROOT_021,
                "the QEMU appliance builder image and digest must not be empty",
            ));
        }
        if !valid_sha512_digest(&self.closure_digest) {
            return Err(Error::new(
                codes::ROOT_021,
                "the QEMU appliance closure digest is not a SHA-512 digest",
            ));
        }
        if self.packages.is_empty() || self.packages.values().all(Vec::is_empty) {
            return Err(Error::new(
                codes::ROOT_021,
                "the QEMU appliance closure contains no packages",
            ));
        }
        validate_package_keys(&self.packages, codes::ROOT_021)?;
        for (package_arch, entries) in &self.packages {
            if package_arch != "noarch" && !self.archs.contains(package_arch) {
                return Err(Error::new(
                    codes::ROOT_021,
                    "the QEMU appliance contains packages for an architecture it does not declare",
                ));
            }
            if entries.is_empty() {
                return Err(Error::new(
                    codes::ROOT_021,
                    "the QEMU appliance contains an empty package group",
                ));
            }
        }
        let expected = closure_digest(
            &self.releasever,
            &self.archs,
            &self.repositories,
            &self.packages,
        );
        if self.closure_digest != expected {
            return Err(Error::new(
                codes::ROOT_021,
                "QEMU appliance closure digest mismatch",
            ));
        }
        Ok(())
    }

    /// Cache identity for the assembled appliance. It changes when the package
    /// closure, schema, or builder image changes, but not when the recipe does.
    pub fn identity(&self) -> String {
        let material = format!(
            "rpmx-appliance/1\nschema:{}\nclosure:{}\nbuilder:{}\n",
            self.schema, self.closure_digest, self.builder.image_digest
        );
        sha512_hex(material.as_bytes())
    }

    /// Present the appliance as a root lock so the existing assembly path can
    /// install it. The result is only used internally; its tree digest is not
    /// stored in the appliance lock.
    pub fn as_root_lock(&self) -> RootLock {
        RootLock {
            document: DOCUMENT.to_string(),
            version: VERSION,
            profile: self.profile.clone(),
            archs: self.archs.clone(),
            releasever: self.releasever.clone(),
            dist_tag: None,
            options: ResolveOptions::default(),
            prefix: crate::profile::default_prefix(),
            libdir: crate::profile::default_libdir(),
            debuginfo: false,
            sbom: false,
            repositories: self.repositories.clone(),
            packages: self.packages.clone(),
            closure_digest: self.closure_digest.clone(),
            root_digest: None,
            builder: Some(self.builder.clone()),
            runner_assets: None,
        }
    }
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
    // Parse the appliance separately first so a malformed or unknown appliance
    // field is reported as ROOT-021, not as a generic root-lock syntax error.
    let mut document: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text)
        .map_err(|error| Error::new(codes::ROOT_006, format!("invalid root lock: {error}")))?;
    if document.get("runner-assets").is_none() {
        if let Some(appliance) = document.get("appliance").cloned() {
            // Legacy locks stored the QEMU appliance directly. Normalise it
            // into the generic runner-assets shape before deserialising.
            if let serde_yaml_ng::Value::Mapping(mapping) = &mut document {
                mapping.remove(&serde_yaml_ng::Value::String("appliance".to_string()));
                let mut assets = serde_yaml_ng::Mapping::new();
                assets.insert(
                    serde_yaml_ng::Value::String("qemu_appliance".to_string()),
                    appliance,
                );
                mapping.insert(
                    serde_yaml_ng::Value::String("runner-assets".to_string()),
                    serde_yaml_ng::Value::Mapping(assets),
                );
            }
        }
    }
    if let Some(assets) = document.get("runner-assets") {
        let parsed: RunnerAssetsLock =
            serde_yaml_ng::from_value(assets.clone()).map_err(|error| {
                Error::new(
                    codes::ROOT_021,
                    format!(
                        "invalid runner assets: {}",
                        sanitize_message(&error.to_string())
                    ),
                )
            })?;
        parsed.validate()?;
    } else if let Some(appliance) = document.get("appliance") {
        // Legacy lock written before runner assets were generalized.
        let parsed: ApplianceLock =
            serde_yaml_ng::from_value(appliance.clone()).map_err(|error| {
                Error::new(
                    codes::ROOT_021,
                    format!(
                        "invalid QEMU appliance: {}",
                        sanitize_message(&error.to_string())
                    ),
                )
            })?;
        parsed.validate()?;
    }
    let normalized = serde_yaml_ng::to_string(&document)
        .map_err(|error| Error::new(codes::ROOT_006, format!("invalid root lock: {error}")))?;
    let lock: RootLock = serde_yaml_ng::from_str(&normalized)
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
    validate_package_keys(&lock.packages, codes::ROOT_006)?;
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
    if let Some(assets) = &lock.runner_assets {
        assets.validate()?;
        if let Some(appliance) = &assets.qemu_appliance {
            validate_architecture_agreement(&lock, appliance)?;
        }
    }
    Ok(lock)
}

pub fn write_lock(path: &Path, lock: &RootLock) -> Result<()> {
    validate_package_keys(&lock.packages, codes::ROOT_006)?;
    if let Some(assets) = &lock.runner_assets {
        assets.validate()?;
        if let Some(appliance) = &assets.qemu_appliance {
            validate_architecture_agreement(lock, appliance)?;
        }
    }
    let text = serde_yaml_ng::to_string(lock)
        .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
    std::fs::write(path, text).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}
