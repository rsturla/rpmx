//! A small cross-process publishing lock for content-addressed cache entries.

use std::io::Write;
use std::path::{Path, PathBuf};

use rpmx_base::error::{codes, Error, Result};

/// A lock file that serialises publishers of one cache key.
///
/// The lock is advisory and stale after two minutes, so a crashed publisher
/// cannot block the cache forever.
pub struct CacheLock {
    path: PathBuf,
}

impl CacheLock {
    pub fn acquire(directory: &Path, key: &str) -> Result<Self> {
        let path = directory.join(format!("{key}.publishing"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > std::time::Duration::from_secs(120));
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(Error::new(
                            codes::ROOT_004,
                            format!(
                                "timed out waiting for another rpmx process to publish {}",
                                directory.display()
                            ),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(error) => {
                    return Err(Error::new(
                        codes::ROOT_004,
                        format!("cannot create {}: {error}", path.display()),
                    ));
                }
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
