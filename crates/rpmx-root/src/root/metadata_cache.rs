//! Copy public remote metadata into each resolver's private DNF cache.
//!
//! Containers never mount this cache. Snapshot readers and publishers hold a
//! short host-side lock, and every copied file has its own inode and label.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::cache_lock::CacheLock;
use crate::profile::Profile;
use rpmx_base::hash::{sha256_file, sha256_hex};

pub(super) struct MetadataCache {
    directory: PathBuf,
    repositories: Vec<(String, String)>,
}

impl MetadataCache {
    pub(super) fn new(profile: &Profile, image_digest: &str, cache: &Path) -> Self {
        let repositories = profile
            .repositories
            .iter()
            .filter_map(|repository| {
                let (kind, location) = repository.location();
                // Credential changes can change the visible repository. Keep
                // authenticated metadata private to the current resolution.
                if repository.auth.is_some()
                    || !(location.starts_with("https://") || location.starts_with("http://"))
                {
                    return None;
                }
                // Filters and package requests do not change repository
                // metadata. Including them would defeat reuse between tests.
                let identity = serde_json::json!([
                    1,
                    profile.arch(),
                    profile.releasever,
                    image_digest,
                    repository.id,
                    kind,
                    location,
                    repository.gpgkey,
                    repository.repo_gpgcheck,
                ]);
                let digest = sha256_hex(identity.to_string().as_bytes());
                Some((repository.id.clone(), digest[7..].to_string()))
            })
            .collect();
        Self {
            directory: cache.join("dnf-metadata-v1"),
            repositories,
        }
    }

    pub(super) fn restore(&self, work: &Path) {
        for (id, key) in &self.repositories {
            if let Err(error) = self.restore_repository(work, key) {
                // A cache miss or damaged snapshot must not prevent a fresh
                // resolution. Remove partial copies before DNF sees them.
                let _ = clear_repository(&work.join("cache"), id);
                rpmx_base::output::warning("metadata", format!("cannot reuse {id}: {error}"));
            }
        }
    }

    pub(super) fn publish(&self, work: &Path) {
        for (id, key) in &self.repositories {
            if let Err(error) = self.publish_repository(work, id, key) {
                rpmx_base::output::warning("metadata", format!("cannot cache {id}: {error}"));
            }
        }
    }

    fn restore_repository(&self, work: &Path, key: &str) -> io::Result<()> {
        if !self.directory.exists() {
            return Ok(());
        }
        let _lock = CacheLock::acquire(&self.directory, key).map_err(io::Error::other)?;
        let directory = self.directory.join(key);
        let pointer = directory.join("current");
        let digest = match std::fs::read_to_string(&pointer) {
            Ok(digest) if valid_digest(&digest) => digest,
            Ok(_) => return Err(io::Error::other("invalid metadata snapshot pointer")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let snapshot = directory.join(&digest);
        if snapshot_digest(&snapshot)? != digest {
            return Err(io::Error::other("metadata snapshot checksum mismatch"));
        }
        copy_tree(&snapshot, &work.join("cache"))
    }

    fn publish_repository(&self, work: &Path, id: &str, key: &str) -> io::Result<()> {
        let entries = repository_entries(&work.join("cache"), id)?;
        if entries.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.directory)?;
        let temporary = tempfile::Builder::new()
            .prefix("snapshot-")
            .tempdir_in(&self.directory)?;
        for entry in entries {
            let destination = temporary.path().join(entry.file_name().unwrap());
            for name in ["repodata", "solv", "metalink.xml", "mirrorlist"] {
                let source = entry.join(name);
                if source.exists() {
                    copy_tree(&source, &destination.join(name))?;
                }
            }
        }
        let digest = snapshot_digest(temporary.path())?;
        let digest = digest.as_str();
        let _lock = CacheLock::acquire(&self.directory, key).map_err(io::Error::other)?;
        let directory = self.directory.join(key);
        std::fs::create_dir_all(&directory)?;
        let destination = directory.join(digest);
        // A damaged snapshot can have the same content-addressed name as
        // freshly downloaded valid metadata. Repair it while holding the
        // reader lock instead of retaining a permanently unusable snapshot.
        if destination.exists()
            && !snapshot_digest(&destination).is_ok_and(|existing| existing == digest)
        {
            std::fs::remove_dir_all(&destination)?;
        }
        if !destination.exists() {
            std::fs::rename(temporary.path(), &destination)?;
        }
        let mut pointer = tempfile::NamedTempFile::new_in(&directory)?;
        pointer.write_all(digest.as_bytes())?;
        pointer.persist(directory.join("current"))?;
        // Readers hold the same lock while copying. Old generations can be
        // removed after the pointer changes without racing a reader.
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name != digest && valid_digest(&name) && entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(())
    }
}

fn snapshot_digest(directory: &Path) -> io::Result<String> {
    let mut manifest = Vec::new();
    tree_manifest(directory, directory, &mut manifest)?;
    manifest.sort();
    let digest = sha256_hex(serde_json::to_string(&manifest)?.as_bytes());
    Ok(digest[7..].to_string())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn repository_entries(cache: &Path, id: &str) -> io::Result<Vec<PathBuf>> {
    let prefix = format!("{id}-");
    let mut result = Vec::new();
    for entry in std::fs::read_dir(cache)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(suffix) = name.strip_prefix(&prefix) else {
            continue;
        };
        if suffix.len() == 16
            && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
            && entry.file_type()?.is_dir()
        {
            result.push(entry.path());
        }
    }
    Ok(result)
}

fn clear_repository(cache: &Path, id: &str) -> io::Result<()> {
    for entry in repository_entries(cache, id)? {
        std::fs::remove_dir_all(entry)?;
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Never hardlink: a resolver container relabels its private /work.
        std::fs::copy(source, destination)?;
    } else {
        return Err(io::Error::other("metadata snapshots require regular files"));
    }
    Ok(())
}

fn tree_manifest(root: &Path, path: &Path, result: &mut Vec<(PathBuf, String)>) -> io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            tree_manifest(root, &entry.path(), result)?;
        } else if file_type.is_file() {
            result.push((
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                sha256_file(&entry.path())?,
            ));
        } else {
            return Err(io::Error::other("metadata snapshots require regular files"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, MetadataExt};

    fn profile() -> Profile {
        serde_yaml_ng::from_str(
            r#"document: rpmx-profile
version: 1
releasever: '44'
archs: [x86_64]
repositories:
  - id: remote
    baseurl: https://example.invalid/repo
  - id: local
    baseurl: file:///tmp/under-test
  - id: private
    baseurl: https://example.invalid/private
    auth:
      username: reader
      password: env:RPMX_TEST_PASSWORD
packages:
  install: [first]
"#,
        )
        .unwrap()
    }

    fn metadata(work: &Path, id: &str, value: &str) -> PathBuf {
        let directory = work.join("cache").join(format!("{id}-1234567890abcdef"));
        std::fs::create_dir_all(directory.join("repodata")).unwrap();
        std::fs::write(directory.join("repodata/repomd.xml"), value).unwrap();
        std::fs::write(directory.join("repodata/primary.xml"), value).unwrap();
        directory
    }

    #[test]
    fn only_metadata_is_shared_and_each_resolver_gets_private_files() {
        let cache = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let source = metadata(first.path(), "remote", "remote metadata");
        metadata(first.path(), "local", "candidate metadata");
        metadata(first.path(), "private", "private metadata");
        std::fs::create_dir_all(source.join("packages")).unwrap();
        std::fs::write(source.join("packages/first.rpm"), "package payload").unwrap();
        std::fs::create_dir(first.path().join("transaction")).unwrap();
        std::fs::write(first.path().join("transaction/transaction.json"), "first").unwrap();
        let snapshots = MetadataCache::new(&profile(), "builder", cache.path());
        snapshots.publish(first.path());
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir(second.path().join("cache")).unwrap();
        snapshots.restore(second.path());
        let restored = second.path().join("cache/remote-1234567890abcdef");
        assert_eq!(
            std::fs::read_to_string(restored.join("repodata/primary.xml")).unwrap(),
            "remote metadata"
        );
        assert!(!restored.join("packages").exists());
        assert!(!second.path().join("transaction").exists());
        assert!(!second.path().join("cache/local-1234567890abcdef").exists());
        assert!(!second
            .path()
            .join("cache/private-1234567890abcdef")
            .exists());
        let source_inode = std::fs::metadata(source.join("repodata/primary.xml"))
            .unwrap()
            .ino();
        let restored_inode = std::fs::metadata(restored.join("repodata/primary.xml"))
            .unwrap()
            .ino();
        assert_ne!(source_inode, restored_inode);
        std::fs::write(restored.join("repodata/primary.xml"), "modified").unwrap();
        let third = tempfile::tempdir().unwrap();
        std::fs::create_dir(third.path().join("cache")).unwrap();
        snapshots.restore(third.path());
        assert_eq!(
            std::fs::read_to_string(
                third
                    .path()
                    .join("cache/remote-1234567890abcdef/repodata/primary.xml")
            )
            .unwrap(),
            "remote metadata"
        );
    }

    #[test]
    fn refreshed_metadata_replaces_the_snapshot_and_old_generations_are_removed() {
        let cache = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        metadata(first.path(), "remote", "version 1");
        let snapshots = MetadataCache::new(&profile(), "builder", cache.path());
        snapshots.publish(first.path());
        metadata(first.path(), "remote", "version 2");
        snapshots.publish(first.path());
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir(second.path().join("cache")).unwrap();
        snapshots.restore(second.path());
        assert_eq!(
            std::fs::read_to_string(
                second
                    .path()
                    .join("cache/remote-1234567890abcdef/repodata/primary.xml")
            )
            .unwrap(),
            "version 2"
        );
        let key = &snapshots.repositories[0].1;
        let generations = std::fs::read_dir(snapshots.directory.join(key))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_type().unwrap().is_dir())
            .count();
        assert_eq!(generations, 1);
    }

    #[test]
    fn filters_and_requests_share_metadata_but_sources_arches_and_builders_do_not() {
        let base = profile();
        let first = MetadataCache::new(&base, "builder", Path::new("cache"));
        let mut changed = base.clone();
        changed.packages.install = vec!["different-package".to_string()];
        changed.repositories[0].exclude = vec!["candidate".to_string()];
        changed.repositories[0].priority = Some(999);
        assert_eq!(
            first.repositories,
            MetadataCache::new(&changed, "builder", Path::new("cache")).repositories
        );
        changed.archs = vec!["aarch64".to_string()];
        assert_ne!(
            first.repositories,
            MetadataCache::new(&changed, "builder", Path::new("cache")).repositories
        );
        changed = base.clone();
        changed.releasever = "45".to_string();
        assert_ne!(
            first.repositories,
            MetadataCache::new(&changed, "builder", Path::new("cache")).repositories
        );
        changed = base.clone();
        changed.repositories[0].baseurl = Some("https://other.invalid".to_string());
        assert_ne!(
            first.repositories,
            MetadataCache::new(&changed, "builder", Path::new("cache")).repositories
        );
        assert_ne!(
            first.repositories,
            MetadataCache::new(&base, "new-builder", Path::new("cache")).repositories
        );
    }

    #[test]
    fn symlink_metadata_is_never_published() {
        let cache = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let source = metadata(first.path(), "remote", "metadata");
        symlink("/etc/passwd", source.join("repodata/credentials")).unwrap();
        let snapshots = MetadataCache::new(&profile(), "builder", cache.path());
        snapshots.publish(first.path());
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir(second.path().join("cache")).unwrap();
        snapshots.restore(second.path());
        assert!(!second.path().join("cache/remote-1234567890abcdef").exists());
    }
    #[test]
    fn corrupt_snapshots_fall_back_and_are_repaired_by_fresh_publication() {
        let cache = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        metadata(first.path(), "remote", "metadata");
        let snapshots = MetadataCache::new(&profile(), "builder", cache.path());
        snapshots.publish(first.path());
        let key = &snapshots.repositories[0].1;
        let directory = snapshots.directory.join(key);
        let digest = std::fs::read_to_string(directory.join("current")).unwrap();
        std::fs::write(
            directory
                .join(digest)
                .join("remote-1234567890abcdef/repodata/primary.xml"),
            "corrupt",
        )
        .unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir(second.path().join("cache")).unwrap();
        snapshots.restore(second.path());
        assert!(!second.path().join("cache/remote-1234567890abcdef").exists());

        // DNF fetched the original valid metadata again after the cache miss.
        // Its digest names the existing corrupt directory, which must be
        // replaced before this root's successful resolution is published.
        metadata(second.path(), "remote", "metadata");
        snapshots.publish(second.path());
        let third = tempfile::tempdir().unwrap();
        std::fs::create_dir(third.path().join("cache")).unwrap();
        snapshots.restore(third.path());
        assert_eq!(
            std::fs::read_to_string(
                third
                    .path()
                    .join("cache/remote-1234567890abcdef/repodata/primary.xml")
            )
            .unwrap(),
            "metadata"
        );
    }

    #[test]
    fn concurrent_publishers_never_mix_metadata_generations() {
        let cache = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            for version in 0..8 {
                let barrier = barrier.clone();
                let cache = cache.path();
                scope.spawn(move || {
                    let work = tempfile::tempdir().unwrap();
                    metadata(work.path(), "remote", &format!("version {version}"));
                    let snapshots = MetadataCache::new(&profile(), "builder", cache);
                    barrier.wait();
                    snapshots.publish(work.path());
                    let copy = tempfile::tempdir().unwrap();
                    std::fs::create_dir(copy.path().join("cache")).unwrap();
                    snapshots.restore(copy.path());
                    let data = copy.path().join("cache/remote-1234567890abcdef/repodata");
                    assert_eq!(
                        std::fs::read(data.join("repomd.xml")).unwrap(),
                        std::fs::read(data.join("primary.xml")).unwrap()
                    );
                });
            }
        });
    }
}
