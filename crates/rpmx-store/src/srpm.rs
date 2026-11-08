use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use rpmx_base::error::{codes, Code, Error, Result};
pub use rpmx_base::hash::sha512_hex;
use serde::{Deserialize, Serialize};

fn default_sbom() -> bool {
    true
}

/// Bundle documents are JSON metadata; a few MiB is already generous.
const MAX_BUNDLE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SnapshotFile {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
    pub digest: String,
}

#[derive(Deserialize)]
struct BundleLock {
    #[serde(default)]
    steps: Vec<LockStep>,
}

#[derive(Deserialize)]
struct BundleManifest {
    #[serde(default)]
    source_date_epoch: u32,
}

#[derive(Debug, Clone)]
pub struct ExtractedBundle {
    pub files: Vec<String>,
    pub steps: BTreeMap<String, String>,
    pub lock_steps: BTreeMap<String, String>,
    pub source_date_epoch: u32,
}

fn read_text_capped(path: &Path, code: Code) -> Result<String> {
    read_text_with_limit(path, MAX_BUNDLE_BYTES, code)
}

fn read_text_with_limit(path: &Path, limit: u64, code: Code) -> Result<String> {
    let file = std::fs::File::open(path)
        .map_err(|error| Error::new(code, format!("cannot open {}: {error}", path.display())))?;
    let metadata = file
        .metadata()
        .map_err(|error| Error::new(code, format!("cannot stat {}: {error}", path.display())))?;
    if !metadata.is_file() {
        return Err(Error::new(
            code,
            format!("{} is not a regular file", path.display()),
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::new(code, format!("cannot read {}: {error}", path.display())))?;
    if bytes.len() as u64 > limit {
        return Err(Error::new(
            code,
            format!(
                "{} exceeds the {limit}-byte bundle-file limit",
                path.display()
            ),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| Error::new(code, format!("{} is not UTF-8", path.display())))
}

/// Read the rpmx documents from an already-extracted SRPM directory.
pub fn read_bundle(directory: &Path, files: Vec<String>) -> Result<ExtractedBundle> {
    let mut steps = BTreeMap::new();
    let steps_path = directory.join("RPMX-STEPS.json");
    if steps_path.exists() {
        let text = read_text_capped(&steps_path, codes::EXTRACT_006)?;
        steps = serde_json::from_str(&text).map_err(|error| {
            Error::new(
                codes::EXTRACT_004,
                format!("invalid {}: {error}", steps_path.display()),
            )
        })?;
    }

    let mut lock_steps = BTreeMap::new();
    let lock_path = directory.join("package.lock.json");
    if lock_path.exists() {
        let text = read_text_capped(&lock_path, codes::EXTRACT_006)?;
        let lock: BundleLock = serde_json::from_str(&text).map_err(|error| {
            Error::new(
                codes::EXTRACT_004,
                format!("invalid {}: {error}", lock_path.display()),
            )
        })?;
        for step in lock.steps {
            if !step.name.is_empty() && !step.digest.is_empty() {
                lock_steps.insert(step.name, step.digest);
            }
        }
    }

    let mut source_date_epoch = 0;
    let manifest_path = directory.join("RPMX-MANIFEST.json");
    if manifest_path.exists() {
        let text = read_text_capped(&manifest_path, codes::EXTRACT_006)?;
        let manifest: BundleManifest = serde_json::from_str(&text).map_err(|error| {
            Error::new(
                codes::EXTRACT_004,
                format!("invalid {}: {error}", manifest_path.display()),
            )
        })?;
        source_date_epoch = manifest.source_date_epoch;
    }

    Ok(ExtractedBundle {
        files,
        steps,
        lock_steps,
        source_date_epoch,
    })
}

pub fn snapshot_dir(dir: &Path) -> Result<Vec<SnapshotFile>> {
    if !dir.is_dir() {
        return Err(Error::new(
            codes::SOURCE_010,
            format!("source directory {} does not exist", dir.display()),
        ));
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<SnapshotFile>) -> Result<()> {
    let entries = rpmx_base::io::sorted_entries(dir, codes::SOURCE_011)?;

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            Error::new(
                codes::SOURCE_011,
                format!("cannot stat {}: {error}", path.display()),
            )
        })?;
        if file_type.is_symlink() {
            return Err(Error::new(
                codes::SOURCE_012,
                format!(
                    "symlink {} is not allowed in a source snapshot",
                    path.display()
                ),
            ));
        }
        if file_type.is_dir() {
            walk(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(Error::new(
                codes::SOURCE_013,
                format!("{} is not a regular file", path.display()),
            ));
        }

        let relative = path.strip_prefix(root).map_err(|error| {
            Error::new(codes::SOURCE_014, format!("cannot relativize: {error}"))
        })?;
        let relative = slash_path(relative)?;
        if relative.contains('/') {
            return Err(Error::new(
                codes::SOURCE_020,
                format!(
                    "source snapshot path '{relative}' is nested; source RPM payloads are flat"
                ),
            ));
        }
        let data = std::fs::read(&path).map_err(|error| {
            Error::new(
                codes::SOURCE_015,
                format!("cannot read {}: {error}", path.display()),
            )
        })?;
        let mode = mode_of(&path)?;
        let digest = sha512_hex(&data);
        files.push(SnapshotFile {
            path: relative,
            data,
            mode,
            digest,
        });
    }
    Ok(())
}

fn slash_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        let text = component.as_os_str().to_str().ok_or_else(|| {
            Error::new(
                codes::SOURCE_016,
                format!("path {} is not valid UTF-8", path.display()),
            )
        })?;
        parts.push(text);
    }
    Ok(parts.join("/"))
}

#[cfg(unix)]
fn mode_of(path: &Path) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path).map_err(|error| {
        Error::new(
            codes::SOURCE_017,
            format!("cannot stat {}: {error}", path.display()),
        )
    })?;
    Ok(metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode_of(_path: &Path) -> Result<u32> {
    Ok(0o644)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LockSource {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LockStep {
    pub name: String,
    pub digest: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LockRoot {
    pub path: String,
    pub digest: String,
}

/// The target half of a cross build: where the sysroot lock lives and which
/// toolchain the build used.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LockCross {
    pub sysroot: LockRoot,
    pub triple: String,
    /// True when the build used the seed's native command names instead of
    /// `<triple>-*` wrappers. Rebuild needs this to reconstruct the toolchain.
    #[serde(default)]
    pub native: bool,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LockBuild {
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub host: Vec<String>,
    #[serde(default)]
    pub target: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default = "default_sbom")]
    pub sbom: bool,
}

#[derive(Clone, Copy)]
pub struct LockBuildRef<'a> {
    pub root_lock: Option<&'a LockRoot>,
    pub cross: Option<&'a LockCross>,
    pub build: &'a LockBuild,
    /// Old root identities replaced by a bootstrap rebuild.
    pub replaced: Option<&'a ReplacedLock>,
}

/// One root identity a bootstrap rebuild replaced.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LockIdentity {
    pub closure_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// Provenance for `rpmx bootstrap rebuild`: the old embedded roots, while
/// `root_lock`/`cross` point at the replacement locks.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReplacedLock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<LockIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross: Option<LockIdentity>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PackageLock {
    pub schema: u32,
    pub recipe_digest: String,
    pub version: String,
    pub release: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_release: Option<u32>,
    pub source_date_epoch: u32,
    pub sources: Vec<LockSource>,
    pub steps: Vec<LockStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_lock: Option<LockRoot>,
    /// Present when the SRPM was produced by a cross build. It names the
    /// embedded sysroot lock, so `rpmx rebuild` needs no profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross: Option<LockCross>,
    pub build: LockBuild,
    /// Present when a bootstrap rebuild replaced an embedded root lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<ReplacedLock>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SourceManifestRecipe {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SourceManifestLock {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SourceManifestSource {
    pub path: String,
    pub digest: String,
    pub size: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SourceManifest {
    pub schema: u32,
    pub generator: String,
    pub source_date_epoch: u32,
    pub recipe: SourceManifestRecipe,
    pub lock: SourceManifestLock,
    pub sources: Vec<SourceManifestSource>,
    pub steps: Vec<LockStep>,
    pub bundled: SourceManifestLock,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_lock: Option<LockRoot>,
}

#[allow(clippy::too_many_arguments)]
pub fn lock_json(
    recipe_digest: &str,
    version: &str,
    release: &str,
    dist_tag: Option<&str>,
    build_release: Option<u32>,
    source_date_epoch: u32,
    sources: &[SnapshotFile],
    steps: &[LockStep],
    context: LockBuildRef<'_>,
) -> Result<String> {
    let value = PackageLock {
        schema: 1,
        recipe_digest: recipe_digest.to_string(),
        version: version.to_string(),
        release: release.to_string(),
        dist_tag: dist_tag.map(str::to_string),
        build_release,
        source_date_epoch,
        sources: sources
            .iter()
            .map(|file| LockSource {
                path: file.path.clone(),
                digest: file.digest.clone(),
            })
            .collect(),
        steps: steps
            .iter()
            .map(|step| LockStep {
                name: step.name.clone(),
                digest: step.digest.clone(),
            })
            .collect(),
        root_lock: context.root_lock.cloned(),
        cross: context.cross.cloned(),
        build: context.build.clone(),
        replaced: context.replaced.cloned(),
    };
    json(value)
}

/// Load the release identity recorded inside an SRPM.
pub fn load_lock_file(path: &Path) -> Result<PackageLock> {
    let text = read_text_capped(path, codes::SRPM_006)?;
    let lock: PackageLock = serde_json::from_str(&text)
        .map_err(|error| Error::new(codes::SRPM_006, format!("invalid package lock: {error}")))?;
    if lock.schema != 1 {
        return Err(Error::new(
            codes::SRPM_006,
            format!(
                "unsupported package lock schema {}; this rpmx understands 1",
                lock.schema
            ),
        ));
    }
    Ok(lock)
}

pub fn manifest_json(
    recipe_digest: &str,
    lock_digest: &str,
    bundled_digest: &str,
    source_date_epoch: u32,
    sources: &[SnapshotFile],
    steps: &[LockStep],
    root_lock: Option<&LockRoot>,
) -> Result<String> {
    let value = SourceManifest {
        schema: 1,
        generator: format!("rpmx {}", env!("CARGO_PKG_VERSION")),
        source_date_epoch,
        recipe: SourceManifestRecipe {
            path: "package.yaml".to_string(),
            digest: recipe_digest.to_string(),
        },
        lock: SourceManifestLock {
            path: "package.lock.json".to_string(),
            digest: lock_digest.to_string(),
        },
        sources: sources
            .iter()
            .map(|file| SourceManifestSource {
                path: file.path.clone(),
                digest: file.digest.clone(),
                size: file.data.len(),
            })
            .collect(),
        steps: steps
            .iter()
            .map(|step| LockStep {
                name: step.name.clone(),
                digest: step.digest.clone(),
            })
            .collect(),
        bundled: SourceManifestLock {
            path: "RPMX-BUNDLED.json".to_string(),
            digest: bundled_digest.to_string(),
        },
        root_lock: root_lock.cloned(),
    };
    json(value)
}

fn json(value: impl Serialize) -> Result<String> {
    let mut text = serde_json::to_string_pretty(&value)
        .map_err(|error| Error::new(codes::SOURCE_018, error.to_string()))?;
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::{read_text_with_limit, LockBuild, LockBuildRef, LockIdentity, ReplacedLock};
    use rpmx_base::error::codes;

    #[test]
    fn replacement_provenance_round_trips() {
        let build = LockBuild::default();
        let replaced = ReplacedLock {
            root: Some(LockIdentity {
                closure_digest: "sha512:old".to_string(),
                root_digest: Some("sha512:old-tree".to_string()),
                profile: Some("stage0".to_string()),
            }),
            cross: None,
        };
        let text = super::lock_json(
            "sha512:recipe",
            "1",
            "1",
            None,
            None,
            1704067200,
            &[],
            &[],
            LockBuildRef {
                root_lock: None,
                cross: None,
                build: &build,
                replaced: Some(&replaced),
            },
        )
        .expect("lock");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["replaced"]["root"]["closure_digest"], "sha512:old");
        assert_eq!(value["replaced"]["root"]["profile"], "stage0");
        let parsed: super::PackageLock = serde_json::from_str(&text).expect("round trip");
        assert!(parsed.replaced.is_some());
    }

    #[test]
    fn bundle_text_over_the_limit_is_rejected() {
        let dir = tempfile::tempdir().expect("work");
        let path = dir.path().join("bundle.json");
        std::fs::write(&path, b"0123456789").expect("bundle");
        let error = read_text_with_limit(&path, 4, codes::EXTRACT_004).expect_err("limit");
        assert_eq!(error.code, "RPMX-EXTRACT-004");
    }

    #[test]
    fn bundle_text_rejects_a_non_regular_file() {
        let dir = tempfile::tempdir().expect("work");
        let path = dir.path().join("bundle.json");
        std::fs::create_dir(&path).expect("directory");
        let error = read_text_with_limit(&path, 4, codes::EXTRACT_004).expect_err("directory");
        assert_eq!(error.code, "RPMX-EXTRACT-004");
    }

    #[test]
    fn bundle_text_at_the_limit_is_allowed() {
        let dir = tempfile::tempdir().expect("work");
        let path = dir.path().join("bundle.json");
        std::fs::write(&path, b"0123").expect("bundle");
        assert_eq!(
            read_text_with_limit(&path, 4, codes::EXTRACT_004).expect("within limit"),
            "0123"
        );
    }
}
