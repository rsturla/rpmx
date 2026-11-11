use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha512};

use rpmx_base::error::{codes, Error, Result};

/// Top-level directories that are always volatile or not part of the root.
const SKIP_TOP: &[&str] = &["dev", "proc", "run", "sys", "tmp"];

/// State written by package managers or at boot, not by the package payload.
///
/// Assembly verifies the installed package set against the lock. Saved roots
/// additionally bind the authoritative database through [`rpmdb_digest`].
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
    digest(root, true, codes::ROOT_014)
}

/// A deterministic digest over a recipe context directory.
///
/// Unlike [`tree_digest`], nothing is skipped: a context is the package's
/// local input, so every file and directory is part of its identity.
pub fn context_digest(root: &Path) -> Result<String> {
    digest(root, false, codes::SOURCE_035)
}

fn digest(root: &Path, skip_volatile: bool, code: rpmx_base::error::Code) -> Result<String> {
    let mut lines = Vec::new();
    walk(root, Path::new(""), &mut lines, skip_volatile, code)?;
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

fn walk(
    root: &Path,
    relative: &Path,
    lines: &mut Vec<String>,
    skip_volatile: bool,
    code: rpmx_base::error::Code,
) -> Result<()> {
    let directory = root.join(relative);
    let entries = rpmx_base::io::sorted_entries(&directory, code)?;

    for entry in entries {
        let name = entry.file_name();
        let child = relative.join(&name);
        let relative_text = child.to_string_lossy().into_owned();
        if skip_volatile && is_skipped(&relative_text) {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            Error::new(code, format!("cannot stat {}: {error}", path.display()))
        })?;
        let mode = metadata.mode() & 0o7777;
        let file_type = metadata.file_type();

        if file_type.is_dir() {
            lines.push(format!("d|{relative_text}|{mode:o}"));
            walk(root, &child, lines, skip_volatile, code)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&path).map_err(|error| {
                Error::new(
                    code,
                    format!("cannot read symlink {}: {error}", path.display()),
                )
            })?;
            lines.push(format!("l|{relative_text}|{}", target.to_string_lossy()));
        } else if file_type.is_file() {
            lines.push(format!(
                "f|{relative_text}|{mode:o}|{}",
                file_digest(&path, code)?
            ));
        }
    }
    Ok(())
}

fn file_digest(path: &Path, code: rpmx_base::error::Code) -> Result<String> {
    let mut file = File::open(path)
        .map_err(|error| Error::new(code, format!("cannot read {}: {error}", path.display())))?;
    let mut hasher = Sha512::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            Error::new(code, format!("cannot read {}: {error}", path.display()))
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

fn digest_root_relaxed(root: &Path) -> Result<String> {
    let mut changed = Vec::new();
    if let Err(error) = make_owner_readable(root, &mut changed) {
        restore_modes(&changed);
        return Err(error);
    }
    let result = tree_digest(root);
    restore_modes(&changed);
    result
}

fn make_owner_readable(path: &Path, changed: &mut Vec<(std::path::PathBuf, u32)>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::symlink_metadata(path).map_err(|error| {
        Error::new(
            codes::ROOT_014,
            format!("cannot stat {}: {error}", path.display()),
        )
    })?;
    let mode = metadata.permissions().mode() & 0o7777;
    let needed = if metadata.is_dir() {
        0o500
    } else if metadata.is_file() {
        0o400
    } else {
        0
    };
    if needed != 0 && mode & needed != needed {
        fs::set_permissions(path, fs::Permissions::from_mode(mode | needed)).map_err(|error| {
            Error::new(
                codes::ROOT_014,
                format!("cannot make {} readable: {error}", path.display()),
            )
        })?;
        changed.push((path.to_path_buf(), mode));
    }
    if metadata.is_dir() {
        let entries = rpmx_base::io::sorted_entries(path, codes::ROOT_014)?;
        for entry in entries {
            make_owner_readable(&entry.path(), changed)?;
        }
    }
    Ok(())
}

fn restore_modes(changed: &[(std::path::PathBuf, u32)]) {
    use std::os::unix::fs::PermissionsExt;

    for (path, mode) in changed.iter().rev() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(*mode));
    }
}

/// Digest a root, re-entering a user namespace when rpm left mode-000 trees
/// behind. The first fallback temporarily grants the owner read access and
/// restores every mode afterwards; this works without namespace privileges
/// when the caller owns the tree. The second fallback re-executes this binary
/// so the walk still sees root.
pub fn digest_root(root: &Path) -> Result<String> {
    match tree_digest(root) {
        Ok(digest) => Ok(digest),
        Err(direct) => {
            if let Ok(digest) = digest_root_relaxed(root) {
                return Ok(digest);
            }
            let executable = std::env::current_exe().map_err(|error| {
                Error::new(
                    codes::ROOT_014,
                    format!("cannot locate this executable: {error}"),
                )
            })?;
            let attempts = [
                Command::new("unshare")
                    .arg("-Ur")
                    .arg("--")
                    .arg(&executable)
                    .arg("root")
                    .arg("digest")
                    .arg(root)
                    .output(),
                Command::new("podman")
                    .arg("unshare")
                    .arg("--")
                    .arg(&executable)
                    .arg("root")
                    .arg("digest")
                    .arg(root)
                    .output(),
            ];
            for output in attempts.into_iter().flatten() {
                if output.status.success() {
                    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if text.starts_with("sha512:") {
                        return Ok(text);
                    }
                }
            }
            Err(direct)
        }
    }
}

/// Bind the installed RPM database without hashing query-created lock or SQLite
/// shared-memory files. WAL and rollback journals contain authoritative state
/// and remain covered. This proof is recorded only after package verification.
pub fn rpmdb_digest(root: &Path) -> Result<String> {
    let root =
        fs::canonicalize(root).map_err(|error| Error::new(codes::ROOT_014, error.to_string()))?;
    let mut records = Vec::new();
    for relative in ["usr/lib/sysimage/rpm", "var/lib/rpm"] {
        let path = root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(_) => database_records(&root, &path, relative, &mut records)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Error::new(
                    codes::ROOT_014,
                    format!("cannot inspect RPM database {}: {error}", path.display()),
                ))
            }
        }
    }
    records.sort();
    let mut hasher = Sha512::new();
    for record in records {
        hasher.update(record.as_bytes());
        hasher.update(b"\n");
    }
    Ok(format!("sha512:{:x}", hasher.finalize()))
}

fn database_records(root: &Path, path: &Path, name: &str, records: &mut Vec<String>) -> Result<()> {
    let resolved = fs::canonicalize(path).map_err(|error| {
        Error::new(
            codes::ROOT_014,
            format!("cannot read RPM database {}: {error}", path.display()),
        )
    })?;
    if !resolved.starts_with(root) {
        return Err(Error::new(
            codes::ROOT_014,
            format!("RPM database path escapes root: {}", path.display()),
        ));
    }
    let metadata = fs::symlink_metadata(&resolved)
        .map_err(|error| Error::new(codes::ROOT_014, error.to_string()))?;
    if metadata.is_dir() {
        records.push(format!("d|{name}"));
        for entry in rpmx_base::io::sorted_entries(&resolved, codes::ROOT_014)? {
            let filename = entry.file_name();
            let filename = filename.to_str().ok_or_else(|| {
                Error::new(codes::ROOT_014, "RPM database contains a non-UTF8 filename")
            })?;
            if filename == ".rpm.lock" || filename.ends_with("-shm") {
                continue;
            }
            if entry
                .file_type()
                .map_err(|error| Error::new(codes::ROOT_014, error.to_string()))?
                .is_symlink()
            {
                return Err(Error::new(
                    codes::ROOT_014,
                    "RPM database contains an unexpected nested symlink",
                ));
            }
            database_records(root, &entry.path(), &format!("{name}/{filename}"), records)?;
        }
    } else if metadata.is_file() {
        records.push(format!(
            "f|{name}|{}",
            file_digest(&resolved, codes::ROOT_014)?
        ));
    } else {
        return Err(Error::new(
            codes::ROOT_014,
            "RPM database contains a special file",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod rpmdb_tests {
    use super::*;

    #[test]
    fn database_proof_covers_headers_and_wal_but_not_query_locks() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("usr/lib/sysimage/rpm");
        fs::create_dir_all(&db).unwrap();
        fs::write(db.join("rpmdb.sqlite"), "headers").unwrap();
        let original = rpmdb_digest(root.path()).unwrap();
        fs::write(db.join(".rpm.lock"), "lock").unwrap();
        fs::write(db.join("rpmdb.sqlite-shm"), "shared memory").unwrap();
        assert_eq!(original, rpmdb_digest(root.path()).unwrap());
        fs::write(db.join("rpmdb.sqlite-wal"), "new transaction").unwrap();
        assert_ne!(original, rpmdb_digest(root.path()).unwrap());
        fs::remove_file(db.join("rpmdb.sqlite-wal")).unwrap();
        fs::write(db.join("rpmdb.sqlite"), "same NEVRA, changed headers").unwrap();
        assert_ne!(original, rpmdb_digest(root.path()).unwrap());
    }

    #[test]
    fn database_aliases_must_remain_inside_root() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("var/lib")).unwrap();
        std::os::unix::fs::symlink("/tmp", root.path().join("var/lib/rpm")).unwrap();
        assert!(rpmdb_digest(root.path())
            .unwrap_err()
            .message
            .contains("escapes root"));
    }
}
