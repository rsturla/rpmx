//! Read package-local test files with stable paths.

use rpmx_base::error::{codes, Error, Result};
use std::path::{Path, PathBuf};

/// A literal path below an owned fixture or result directory.
pub fn validate_relative(path: &str) -> Result<()> {
    if path
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || path
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\\' | '*' | '?' | '[' | ']'))
    {
        return Err(Error::new(codes::TEST_004,
            "fixture and artifact paths must be literal relative paths without empty, '.' or '..' components"));
    }
    Ok(())
}

pub fn entries(recipe_dir: &Path, directory: &str) -> Result<Vec<PathBuf>> {
    validate_relative(directory)?;
    let relative = Path::new(directory);
    let mut path = recipe_dir.to_path_buf();
    for part in relative.components() {
        path.push(part);
        if metadata(&path)?.file_type().is_symlink() {
            return Err(Error::new(
                codes::TEST_004,
                "test files cannot contain symlinks",
            ));
        }
    }
    if !path.is_dir() {
        return Err(Error::new(
            codes::TEST_004,
            format!("test files {} is not a directory", path.display()),
        ));
    }
    let mut files = Vec::new();
    walk(&path, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(Error::new(
            codes::TEST_004,
            "test files directory must contain at least one file",
        ));
    }
    Ok(files)
}

fn metadata(path: &Path) -> Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path).map_err(|error| {
        Error::new(
            codes::TEST_004,
            format!("cannot read test file {}: {error}", path.display()),
        )
    })
}

fn walk(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        std::fs::read_dir(path).map_err(|error| Error::new(codes::TEST_004, error.to_string()))?
    {
        let path = entry
            .map_err(|error| Error::new(codes::TEST_004, error.to_string()))?
            .path();
        let kind = metadata(&path)?.file_type();
        if kind.is_dir() {
            walk(&path, files)?;
        } else if kind.is_file() {
            files.push(path);
        } else {
            return Err(Error::new(
                codes::TEST_004,
                format!(
                    "test file {} must be a regular file or directory",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_recursive_and_sorted() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("tests/include")).unwrap();
        std::fs::write(directory.path().join("tests/z.c"), "").unwrap();
        std::fs::write(directory.path().join("tests/include/a.h"), "").unwrap();
        let files = entries(directory.path(), "tests").unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("tests/include/a.h"));
        for invalid in ["", ".", "../tests", "/tests", "tests/../tests"] {
            assert!(entries(directory.path(), invalid).is_err(), "{invalid}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_in_files_and_parent_paths_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("tests")).unwrap();
        std::os::unix::fs::symlink("tests", directory.path().join("alias")).unwrap();
        assert!(entries(directory.path(), "alias").is_err());
        std::os::unix::fs::symlink("/etc/passwd", directory.path().join("tests/link")).unwrap();
        assert!(entries(directory.path(), "tests").is_err());
    }
}
