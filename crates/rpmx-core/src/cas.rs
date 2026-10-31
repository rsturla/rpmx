use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::{codes, Error, Result};
use crate::srpm::sha512_hex;

/// The on-disk name for a content-addressed payload.
pub fn path(cache: &Path, digest: &str) -> Result<PathBuf> {
    let hex = digest.strip_prefix("sha512:").ok_or_else(|| {
        Error::new(
            codes::ROOT_011,
            format!("digest '{digest}' is not a sha512 digest"),
        )
    })?;
    if hex.len() != 128 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new(
            codes::ROOT_011,
            format!("digest '{digest}' is not a sha512 digest"),
        ));
    }
    Ok(cache.join("sha512").join(hex))
}

pub fn store(cache: &Path, bytes: &[u8]) -> Result<String> {
    let digest = sha512_hex(bytes);
    let target = path(cache, &digest)?;
    if target.exists() {
        return Ok(digest);
    }
    let parent = target.parent().expect("CAS path has a parent");
    std::fs::create_dir_all(parent).map_err(|error| {
        Error::new(
            codes::ROOT_011,
            format!("cannot create {}: {error}", parent.display()),
        )
    })?;
    let temporary = target.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&temporary, bytes).map_err(|error| {
        Error::new(
            codes::ROOT_011,
            format!("cannot write {}: {error}", temporary.display()),
        )
    })?;
    std::fs::rename(&temporary, &target).map_err(|error| {
        Error::new(
            codes::ROOT_011,
            format!("cannot publish {}: {error}", target.display()),
        )
    })?;
    Ok(digest)
}

pub struct GcOptions<'a> {
    /// Digests that must stay, usually read from root locks and recipes.
    pub keep: &'a BTreeSet<String>,
    /// Keep blobs younger than this, so a concurrent fetch is not deleted.
    pub older_than: Duration,
    pub dry_run: bool,
    pub now: SystemTime,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub kept: usize,
    pub removed: usize,
    pub bytes: u64,
    /// Removed (or, with `dry_run`, removable) digests.
    pub blobs: Vec<String>,
}

/// Remove unreferenced payloads from the cache. A digest is retained when it is
/// in `keep`, when it is younger than `older_than`, or when the file is not a
/// well-formed SHA-512 blob.
pub fn gc(cache: &Path, options: GcOptions<'_>) -> Result<GcReport> {
    let directory = cache.join("sha512");
    let mut report = GcReport::default();
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(error) => {
            return Err(Error::new(
                codes::CACHE_001,
                format!("cannot read {}: {error}", directory.display()),
            ));
        }
    };

    for entry in entries {
        let entry = entry.map_err(|error| {
            Error::new(
                codes::CACHE_001,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.len() != 128 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let digest = format!("sha512:{name}");
        let metadata = entry.metadata().map_err(|error| {
            Error::new(
                codes::CACHE_001,
                format!("cannot stat {}: {error}", entry.path().display()),
            )
        })?;
        if !metadata.is_file() || options.keep.contains(&digest) {
            report.kept += 1;
            continue;
        }
        let recent = metadata
            .modified()
            .ok()
            .and_then(|modified| options.now.duration_since(modified).ok())
            .is_some_and(|age| age < options.older_than);
        if recent {
            report.kept += 1;
            continue;
        }
        if !options.dry_run {
            std::fs::remove_file(entry.path()).map_err(|error| {
                Error::new(
                    codes::CACHE_002,
                    format!("cannot remove {}: {error}", entry.path().display()),
                )
            })?;
        }
        report.removed += 1;
        report.bytes += metadata.len();
        report.blobs.push(digest);
    }
    Ok(report)
}

pub fn load(cache: &Path, digest: &str) -> Result<Vec<u8>> {
    let target = path(cache, digest)?;
    let bytes = std::fs::read(&target).map_err(|error| {
        Error::new(
            codes::ROOT_011,
            format!(
                "payload {digest} is not in the cache ({}): {error}",
                target.display()
            ),
        )
    })?;
    let actual = sha512_hex(&bytes);
    if actual != digest {
        return Err(Error::new(
            codes::ROOT_011,
            format!("payload {digest} is corrupt: cached content is {actual}"),
        ));
    }
    Ok(bytes)
}
