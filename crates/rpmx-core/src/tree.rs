use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use sha2::{Digest, Sha512};

use crate::error::{codes, Error, Result};

/// Top-level directories that are always volatile or not part of the root.
const SKIP_TOP: &[&str] = &["dev", "proc", "run", "sys", "tmp"];

/// State written by package managers or at boot, not by the package payload.
///
/// The installed package set is verified separately against the lock, so
/// excluding the rpmdb does not weaken the closure check.
const SKIP: &[&str] = &[
    "etc/machine-id",
    // JKS trust stores embed salt, so update-ca-trust produces a different
    // file on every run. The PEM bundles next to it stay covered.
    "etc/pki/ca-trust/extracted/java/cacerts",
    "usr/lib/sysimage/libdnf5",
    "usr/lib/sysimage/rpm",
    "var/cache",
    "var/lib/dnf",
    "var/lib/rpm",
    "var/lib/systemd/random-seed",
    "var/log",
];

/// A deterministic digest over an assembled root.
///
/// Covers paths, file type, mode, symlink targets and file contents.
///
/// Ownership is deliberately excluded: a user namespace reports the same file
/// with a different uid/gid depending on how it was entered, so including it
/// would make the digest depend on the reader. Ownership and capabilities are
/// pinned by the package headers and are a candidate for a future lock field.
///
/// Mtimes are excluded: package payloads carry stable times, but directories
/// created by scriptlets do not. Special files are not represented.
pub fn tree_digest(root: &Path) -> Result<String> {
    let mut lines = Vec::new();
    walk(root, Path::new(""), &mut lines)?;
    // Global ordering, not traversal ordering: the digest must not depend on
    // how a directory's entries happen to be interleaved with its children.
    lines.sort();
    let mut hasher = Sha512::new();
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    Ok(format!("sha512:{:x}", hasher.finalize()))
}

fn walk(root: &Path, relative: &Path, lines: &mut Vec<String>) -> Result<()> {
    let directory = root.join(relative);
    let mut entries = fs::read_dir(&directory)
        .map_err(|error| {
            Error::new(
                codes::ROOT_014,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            Error::new(
                codes::ROOT_014,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?;
    entries.sort_by(|left, right| {
        left.file_name()
            .as_bytes()
            .cmp(right.file_name().as_bytes())
    });

    for entry in entries {
        let name = entry.file_name();
        let child = relative.join(&name);
        let relative_text = child.to_string_lossy().into_owned();
        if is_skipped(&relative_text) {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            Error::new(
                codes::ROOT_014,
                format!("cannot stat {}: {error}", path.display()),
            )
        })?;
        let mode = metadata.mode() & 0o7777;
        let file_type = metadata.file_type();

        if file_type.is_dir() {
            lines.push(format!("d|{relative_text}|{mode:o}"));
            walk(root, &child, lines)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&path).map_err(|error| {
                Error::new(
                    codes::ROOT_014,
                    format!("cannot read symlink {}: {error}", path.display()),
                )
            })?;
            lines.push(format!("l|{relative_text}|{}", target.to_string_lossy()));
        } else if file_type.is_file() {
            lines.push(format!(
                "f|{relative_text}|{mode:o}|{}",
                file_digest(&path)?
            ));
        }
    }
    Ok(())
}

fn file_digest(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|error| {
        Error::new(
            codes::ROOT_014,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let mut hasher = Sha512::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            Error::new(
                codes::ROOT_014,
                format!("cannot read {}: {error}", path.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn is_skipped(relative: &str) -> bool {
    let top = relative.split('/').next().unwrap_or(relative);
    if SKIP_TOP.contains(&top) {
        return true;
    }
    SKIP.iter().any(|prefix| {
        relative == *prefix
            || relative
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}
