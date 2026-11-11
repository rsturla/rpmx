use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::profile::{normalize_arch, Repository, ResolveOptions};
use rpmx_base::error::{codes, Error, Result};
use rpmx_base::hash::sha512_hex;

pub const DOCUMENT: &str = "rpmx-root-lock";
pub const VERSION: u32 = 2;

/// Version of the runner-owned appliance package set and boot protocol.
///
/// Bump this when `APPLIANCE_PACKAGES` or the QEMU boot/mount contract changes.
/// An old lock is then rejected with a re-resolve hint instead of booting an
/// appliance that no longer matches the runner.
pub const APPLIANCE_SCHEMA: u32 = 3;

/// A resolved build root: repositories, an exact package closure and the
/// builder that produced it.
///
/// Every package has an immutable payload digest and explicit origin, including
/// locally supplied candidates. The lock is the entire installation authority.
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
    #[serde(default = "crate::profile::default_sbom")]
    pub sbom: bool,
    pub repositories: Vec<Repository>,
    pub packages: BTreeMap<String, Vec<LockedPackage>>,
    pub closure_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_digest: Option<String>,
    /// Authoritative installed-database bytes, bound after assembly verifies
    /// the package closure. SQLite shared-memory and lock files are transient.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpmdb_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builder: Option<LockedBuilder>,
    /// Seed provenance recorded by a bootstrap stage. It is metadata, outside
    /// `closure_digest`, so a new generation label does not invalidate a root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    /// Seed generation recorded by a bootstrap stage, outside `closure_digest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    /// Digest of the effective bootstrap requirement (profile plus stage
    /// extras). It lets a later stage run reuse a root without re-resolving and
    /// is deliberately outside `closure_digest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement: Option<String>,
}

impl RootLock {
    /// Public provenance retains the exact package and tool-image bindings,
    /// while credential references remain in the private acquisition policy.
    pub fn redacted(&self) -> Self {
        let mut public = self.clone();
        for repository in &mut public.repositories {
            repository.auth = None;
        }
        public
    }

    pub fn validate(&self) -> Result<()> {
        if self.document != DOCUMENT || self.version != VERSION {
            return Err(Error::new(
                codes::ROOT_006,
                format!("expected {DOCUMENT} version {VERSION}; regenerate the root lock"),
            ));
        }
        if self.archs.len() != 1 {
            return Err(Error::new(
                codes::ROOT_006,
                "a root lock must describe exactly one target architecture",
            ));
        }
        let arch = normalize_arch(&self.archs[0])?;
        validate_package_keys(&self.packages, codes::ROOT_006)?;
        if self.packages.values().all(Vec::is_empty) {
            return Err(Error::new(
                codes::ROOT_006,
                "root lock has no package closure",
            ));
        }
        let mut identities = std::collections::BTreeSet::new();
        for (package_arch, packages) in &self.packages {
            if package_arch != "noarch" && package_arch != arch {
                return Err(Error::new(
                    codes::ROOT_006,
                    "root lock includes a foreign package architecture",
                ));
            }
            for package in packages {
                if !identities.insert((&package.name, package_arch)) {
                    return Err(Error::new(
                        codes::ROOT_006,
                        "root lock contains duplicate package identities",
                    ));
                }
                if let PackageOrigin::Repository { repository, .. } = &package.origin {
                    if !self
                        .repositories
                        .iter()
                        .any(|entry| entry.id == *repository)
                    {
                        return Err(Error::new(
                            codes::ROOT_006,
                            format!("unknown package repository '{repository}'"),
                        ));
                    }
                }
            }
        }
        if let Some(builder) = &self.builder {
            if builder.installer.architecture() != arch {
                return Err(Error::new(
                    codes::ROOT_006,
                    "installer image architecture differs from root target",
                ));
            }
        }
        let expected = closure_digest(
            &self.releasever,
            &self.archs,
            &self.repositories,
            &self.packages,
        );
        if expected != self.closure_digest {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "root lock closure digest mismatch: recorded {}, recomputed {expected}",
                    self.closure_digest
                ),
            ));
        }
        Ok(())
    }

    /// Cache identity for the complete installation, distinct from package closure identity.
    pub fn assembly_identity(&self) -> String {
        let material = serde_json::json!({
            "schema": 2,
            "closure": self.closure_digest,
            "target": self.archs,
            "options": self.options,
            "prefix": self.prefix,
            "libdir": self.libdir,
            "installer": self.builder.as_ref().map(|builder| &builder.installer),
        });
        sha512_hex(&serde_json::to_vec(&material).expect("root identity serializes"))
    }

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
    lock: RootLock,
    path: std::path::PathBuf,
    digest: String,
    _temporary: Option<tempfile::TempDir>,
}

impl RootSelection {
    pub fn verify(lock: &RootLock, root: &Path, arch: &str) -> Result<Self> {
        lock.validate()?;
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
        let expected_database = lock.rpmdb_digest.as_deref().ok_or_else(|| {
            Error::new(
                codes::ROOT_017,
                "saved root lock has no rpmdb_digest; assemble it again",
            )
        })?;
        let database = rpmx_store::tree::rpmdb_digest(&path)?;
        if database != expected_database {
            return Err(Error::new(codes::ROOT_017, format!(
                "installed RPM database differs from the assembled lock: expected {expected_database}, found {database}"
            )));
        }
        Ok(Self {
            lock: lock.clone(),
            path,
            digest,
            _temporary: None,
        })
    }

    pub fn lock(&self) -> &RootLock {
        &self.lock
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn assemble(mut lock: RootLock, options: super::AssembleOptions<'_>) -> Result<Self> {
        let path = super::assemble(&lock, options)?;
        let digest = rpmx_store::tree::digest_root(&path)?;
        lock.root_digest = Some(digest.clone());
        lock.rpmdb_digest = Some(rpmx_store::tree::rpmdb_digest(&path)?);
        Ok(Self {
            lock,
            path,
            digest,
            _temporary: None,
        })
    }

    pub fn temporary(lock: RootLock, cache: &Path, work: &Path) -> Result<Self> {
        let temporary = tempfile::Builder::new()
            .prefix("rpmx-root-")
            .tempdir()
            .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
        let root = temporary.path().join("root");
        let mut selection = Self::assemble(
            lock,
            super::AssembleOptions {
                cache,
                work,
                root: &root,
            },
        )?;
        selection._temporary = Some(temporary);
        Ok(selection)
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

impl Drop for RootSelection {
    fn drop(&mut self) {
        if self._temporary.is_some() && self.path.exists() {
            let _ = super::remove_tree(&self.path);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedBuilder {
    pub resolver: LockedImage,
    pub installer: LockedImage,
}

/// A Linux platform manifest, never an OCI index or a mutable tag alone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "ImageRecord", into = "ImageRecord")]
pub struct LockedImage {
    reference: String,
    digest: String,
    architecture: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageRecord {
    reference: String,
    digest: String,
    architecture: String,
}

impl LockedImage {
    pub fn new(reference: String, digest: String, architecture: &str) -> Result<Self> {
        let architecture = normalize_arch(architecture)?.to_string();
        if reference.trim().is_empty()
            || reference.chars().any(char::is_whitespace)
            || !digest.strip_prefix("sha256:").is_some_and(|hex| {
                hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(Error::new(
                codes::ROOT_006,
                "builder requires an image reference and a SHA-256 platform manifest digest",
            ));
        }
        Ok(Self {
            reference,
            digest,
            architecture,
        })
    }
    pub fn reference(&self) -> &str {
        &self.reference
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn architecture(&self) -> &str {
        &self.architecture
    }
    pub fn pinned_reference(&self) -> String {
        super::container::locked_image_reference(&self.reference, &self.digest)
    }
}

impl TryFrom<ImageRecord> for LockedImage {
    type Error = String;
    fn try_from(value: ImageRecord) -> std::result::Result<Self, Self::Error> {
        Self::new(value.reference, value.digest, &value.architecture).map_err(|error| error.message)
    }
}
impl From<LockedImage> for ImageRecord {
    fn from(value: LockedImage) -> Self {
        Self {
            reference: value.reference,
            digest: value.digest,
            architecture: value.architecture,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PackageOrigin {
    Repository {
        repository: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<String>,
    },
    /// Immutable RPM payload supplied by the caller and retained in CAS.
    Candidate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub name: String,
    pub origin: PackageOrigin,
    pub evr: String,
    pub checksum: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub srpm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
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

fn valid_sha512_digest(value: &str) -> bool {
    value
        .strip_prefix("sha512:")
        .is_some_and(|hex| hex.len() == 128 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Payloads imported or downloaded by rpmx use complete SHA-512 CAS identities.
fn valid_package_checksum(value: &str) -> bool {
    valid_sha512_digest(value)
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
        if self.releasever.trim().is_empty() || self.profile.chars().any(char::is_control) {
            return Err(Error::new(
                codes::ROOT_021,
                "the QEMU appliance releasever, profile, or builder image is invalid",
            ));
        }
        if self.builder.installer.architecture() != self.archs[0] {
            return Err(Error::new(
                codes::ROOT_021,
                "appliance installer image has the wrong architecture",
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
            "rpmx-appliance/2\nschema:{}\nclosure:{}\nbuilder:{}\n",
            self.schema,
            self.closure_digest,
            self.builder.installer.digest()
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
            sbom: false,
            repositories: self.repositories.clone(),
            packages: self.packages.clone(),
            closure_digest: self.closure_digest.clone(),
            root_digest: None,
            rpmdb_digest: None,
            builder: Some(self.builder.clone()),
            seed: None,
            generation: None,
            requirement: None,
        }
    }
}

/// The reproducible input: repositories plus the exact package closure.
///
/// Volatile fields (`root_digest` and builder image) are deliberately
/// excluded so two resolutions of the same inputs agree.
pub fn closure_digest(
    releasever: &str,
    archs: &[String],
    repositories: &[Repository],
    packages: &BTreeMap<String, Vec<LockedPackage>>,
) -> String {
    let mut material = String::from("rpmx-root-lock/2\n");
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
        if !repository.exclude.is_empty() {
            let mut exclude = repository.exclude.clone();
            exclude.sort();
            material.push_str(&format!(
                "exclude:{}:{}\n",
                repository.id,
                exclude.join(",")
            ));
        }
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
                serde_json::to_string(&package.origin).expect("package origin serializes"),
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
    lock.validate()?;
    Ok(lock)
}

pub fn write_lock(path: &Path, lock: &RootLock) -> Result<()> {
    lock.validate()?;
    let text = serde_yaml_ng::to_string(lock)
        .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
    std::fs::write(path, text).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}

#[cfg(test)]
mod closure_digest_tests {
    use super::closure_digest;
    use crate::profile::Repository;
    use std::collections::BTreeMap;

    fn repository(id: &str) -> Repository {
        Repository {
            id: id.to_string(),
            baseurl: Some(format!("https://example.invalid/{id}")),
            metalink: None,
            mirrorlist: None,
            gpgkey: None,
            repo_gpgcheck: false,
            exclude: Vec::new(),
            priority: None,
            auth: None,
        }
    }

    #[test]
    fn an_empty_exclude_does_not_change_the_closure_digest() {
        let plain = repository("fedora");
        let mut empty = repository("fedora");
        empty.exclude = Vec::new();
        assert_eq!(
            closure_digest("44", &["x86_64".to_string()], &[plain], &BTreeMap::new()),
            closure_digest("44", &["x86_64".to_string()], &[empty], &BTreeMap::new())
        );
    }

    #[test]
    fn a_non_empty_exclude_changes_the_closure_digest() {
        let plain = repository("fedora");
        let mut filtered = repository("fedora");
        filtered.exclude = vec!["gcc".to_string(), "glibc".to_string()];
        assert_ne!(
            closure_digest("44", &["x86_64".to_string()], &[plain], &BTreeMap::new()),
            closure_digest("44", &["x86_64".to_string()], &[filtered], &BTreeMap::new())
        );
    }
}

#[cfg(test)]
pub(crate) mod model_tests {
    use super::*;

    pub(crate) fn candidate_lock(package: &super::super::Candidate) -> RootLock {
        let packages = BTreeMap::from([(
            "noarch".to_string(),
            vec![LockedPackage {
                name: package.metadata().name.clone(),
                evr: package.metadata().evr(),
                origin: PackageOrigin::Candidate,
                checksum: package.checksum().to_string(),
                size: package.size(),
                srpm: None,
                module: None,
            }],
        )]);
        let archs = vec!["x86_64".to_string()];
        RootLock {
            document: DOCUMENT.to_string(),
            version: VERSION,
            profile: "candidate-control".to_string(),
            archs: archs.clone(),
            releasever: "44".to_string(),
            dist_tag: None,
            options: Default::default(),
            prefix: "/usr".to_string(),
            libdir: "/usr/lib64".to_string(),
            sbom: false,
            repositories: vec![],
            closure_digest: closure_digest("44", &archs, &[], &packages),
            packages,
            root_digest: None,
            rpmdb_digest: None,
            builder: None,
            seed: None,
            generation: None,
            requirement: None,
        }
    }

    #[test]
    fn candidate_origin_round_trips_without_original_paths_or_a_fake_repository() {
        let work = tempfile::tempdir().unwrap();
        let path = super::super::candidate::tests::rpm(work.path(), "fixture", "1", "noarch", None);
        let candidates = super::super::CandidateSet::import(work.path(), &[path]).unwrap();
        let lock = candidate_lock(&candidates.packages()[0]);
        let path = work.path().join("root.lock.yaml");
        write_lock(&path, &lock).unwrap();
        let restored = load_lock(&path).unwrap();
        assert!(matches!(
            restored.packages["noarch"][0].origin,
            PackageOrigin::Candidate
        ));
        assert!(restored.repositories.is_empty());
        assert_eq!(restored.closure_digest, lock.closure_digest);
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains(work.path().to_str().unwrap()));
        assert!(crate::pkgmanifest::from_lock(&restored).is_err());
    }

    #[test]
    fn saved_root_verification_checks_both_tree_and_full_lock() {
        let work = tempfile::tempdir().unwrap();
        let path = super::super::candidate::tests::rpm(work.path(), "fixture", "1", "noarch", None);
        let candidates = super::super::CandidateSet::import(work.path(), &[path]).unwrap();
        let mut lock = candidate_lock(&candidates.packages()[0]);
        let root = work.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("value"), "42").unwrap();
        lock.root_digest = Some(rpmx_store::tree::digest_root(&root).unwrap());
        lock.rpmdb_digest = Some(rpmx_store::tree::rpmdb_digest(&root).unwrap());
        assert!(RootSelection::verify(&lock, &root, "x86_64").is_ok());
        assert!(RootSelection::verify(&lock, &root, "aarch64").is_err());
        let mut forged = lock.clone();
        forged.packages.get_mut("noarch").unwrap()[0].checksum =
            format!("sha512:{}", "a".repeat(128));
        assert!(RootSelection::verify(&forged, &root, "x86_64").is_err());
        let database = root.join("usr/lib/sysimage/rpm");
        std::fs::create_dir_all(&database).unwrap();
        std::fs::write(database.join("rpmdb.sqlite"), "changed headers").unwrap();
        // Existing parent directories affect the tree proof too; bind that tree
        // to isolate the database-specific failure in this unit control.
        lock.root_digest = Some(rpmx_store::tree::digest_root(&root).unwrap());
        let error = RootSelection::verify(&lock, &root, "x86_64").err().unwrap();
        assert!(error.message.contains("installed RPM database differs"));
        lock.rpmdb_digest = Some(rpmx_store::tree::rpmdb_digest(&root).unwrap());
        assert!(RootSelection::verify(&lock, &root, "x86_64").is_ok());
        std::fs::write(root.join("value"), "43").unwrap();
        assert!(RootSelection::verify(&lock, &root, "x86_64").is_err());
        let mut old = lock;
        old.version = 1;
        assert!(old.validate().unwrap_err().message.contains("regenerate"));
    }

    #[test]
    fn installation_identity_tracks_policy_and_installer_but_not_tree_or_resolver() {
        let work = tempfile::tempdir().unwrap();
        let input =
            super::super::candidate::tests::rpm(work.path(), "fixture", "1", "noarch", None);
        let candidates = super::super::CandidateSet::import(work.path(), &[input]).unwrap();
        let mut lock = candidate_lock(&candidates.packages()[0]);
        let image = LockedImage::new(
            "registry/image:tag".into(),
            format!("sha256:{}", "a".repeat(64)),
            "x86_64",
        )
        .unwrap();
        lock.builder = Some(LockedBuilder {
            resolver: image.clone(),
            installer: image,
        });
        let identity = lock.assembly_identity();
        let mut changed = lock.clone();
        changed.options.docs = !changed.options.docs;
        assert_ne!(changed.assembly_identity(), identity);
        let mut changed = lock.clone();
        changed.libdir = "/usr/lib".into();
        assert_ne!(changed.assembly_identity(), identity);
        let replacement = LockedImage::new(
            "registry/image:tag".into(),
            format!("sha256:{}", "b".repeat(64)),
            "x86_64",
        )
        .unwrap();
        let mut changed = lock.clone();
        changed.builder.as_mut().unwrap().installer = replacement.clone();
        assert_ne!(changed.assembly_identity(), identity);
        let mut changed = lock;
        changed.builder.as_mut().unwrap().resolver = replacement;
        changed.root_digest = Some(format!("sha512:{}", "a".repeat(128)));
        changed.rpmdb_digest = Some(format!("sha512:{}", "b".repeat(128)));
        changed.profile = "another-provenance-name".into();
        assert_eq!(changed.assembly_identity(), identity);
    }

    #[test]
    fn locked_images_reject_invalid_identity_and_preserve_registry_ports() {
        assert!(LockedImage::new("image".into(), "sha256:index".into(), "x86_64").is_err());
        assert!(LockedImage::new(
            "image".into(),
            format!("sha256:{}", "a".repeat(64)),
            "other"
        )
        .is_err());
        let image = LockedImage::new(
            "docker://localhost:5000/org/image:tag".into(),
            format!("sha256:{}", "a".repeat(64)),
            "amd64",
        )
        .unwrap();
        assert_eq!(image.architecture(), "x86_64");
        assert_eq!(
            image.pinned_reference(),
            format!(
                "docker://localhost:5000/org/image@sha256:{}",
                "a".repeat(64)
            )
        );
        let mut encoded = serde_json::to_value(&image).unwrap();
        encoded["digest"] = serde_json::json!("mutable");
        assert!(serde_json::from_value::<LockedImage>(encoded).is_err());
    }
}
