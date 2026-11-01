use std::path::Path;

use crate::error::{codes, Error, Result};
pub use crate::hash::sha512_hex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct SnapshotFile {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
    pub digest: String,
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
    let mut entries = std::fs::read_dir(dir)
        .map_err(|error| {
            Error::new(
                codes::SOURCE_011,
                format!("cannot read {}: {error}", dir.display()),
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            Error::new(
                codes::SOURCE_011,
                format!("cannot read {}: {error}", dir.display()),
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());

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
    #[serde(default)]
    pub debuginfo: bool,
    #[serde(default = "crate::profile::default_sbom")]
    pub sbom: bool,
}

#[derive(Clone, Copy)]
pub struct LockBuildRef<'a> {
    pub root_lock: Option<&'a LockRoot>,
    pub build: &'a LockBuild,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LockFile {
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
    pub build: LockBuild,
}

#[derive(Debug, Serialize)]
pub struct ManifestRecipe {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Serialize)]
pub struct ManifestLock {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Serialize)]
pub struct ManifestSource {
    pub path: String,
    pub digest: String,
    pub size: usize,
}

#[derive(Debug, Serialize)]
pub struct Manifest {
    pub schema: u32,
    pub generator: String,
    pub source_date_epoch: u32,
    pub recipe: ManifestRecipe,
    pub lock: ManifestLock,
    pub sources: Vec<ManifestSource>,
    pub steps: Vec<LockStep>,
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
    let value = LockFile {
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
        build: context.build.clone(),
    };
    json(value)
}

/// Load the release identity recorded inside an SRPM.
pub fn load_lock_file(path: &Path) -> Result<LockFile> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::SRPM_006,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let lock: LockFile = serde_json::from_str(&text)
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
    source_date_epoch: u32,
    sources: &[SnapshotFile],
    steps: &[LockStep],
    root_lock: Option<&LockRoot>,
) -> Result<String> {
    let value = Manifest {
        schema: 1,
        generator: format!("rpmx {}", env!("CARGO_PKG_VERSION")),
        source_date_epoch,
        recipe: ManifestRecipe {
            path: "package.yaml".to_string(),
            digest: recipe_digest.to_string(),
        },
        lock: ManifestLock {
            path: "package.lock.json".to_string(),
            digest: lock_digest.to_string(),
        },
        sources: sources
            .iter()
            .map(|file| ManifestSource {
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
