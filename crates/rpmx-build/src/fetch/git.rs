use std::path::Path;

use rpmx_base::error::{codes, Error, Result};
use rpmx_language::model::{Locator, Source};

pub(crate) fn git_archive(
    source: &Source,
    locator: &Locator,
    cache: &Path,
    temp_dir: &Path,
) -> Result<tempfile::TempPath> {
    let url = locator.url.as_deref().ok_or_else(|| {
        Error::new(
            codes::FETCH_002,
            format!("git locator for '{}' has no url", source.name),
        )
    })?;
    let commit = locator.commit.as_deref().ok_or_else(|| {
        Error::new(
            codes::FETCH_002,
            format!("git locator for '{}' has no commit", source.name),
        )
    })?;
    let scratch = cache
        .join("tmp")
        .join(format!("git-{}-{}", std::process::id(), source.name));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).ok();
    }
    std::fs::create_dir_all(&scratch).map_err(|error| {
        Error::new(
            codes::FETCH_003,
            format!("cannot create {}: {error}", scratch.display()),
        )
    })?;
    let result = (|| {
        run_git(&scratch, &["init", "-q"])?;
        run_git(&scratch, &["remote", "add", "origin", url])?;
        let fetch = std::process::Command::new("git")
            .arg("-C")
            .arg(&scratch)
            .args(["fetch", "-q", "--depth=1", "origin", commit])
            .output()
            .map_err(|error| Error::new(codes::FETCH_003, format!("cannot run git: {error}")))?;
        if !fetch.status.success() {
            // Some servers refuse fetching an arbitrary commit; fetch the
            // configured ref (or the default ref) and check out the commit.
            let reference = locator.reference.as_deref().unwrap_or("HEAD");
            run_git(&scratch, &["fetch", "-q", "--depth=1", "origin", reference])?;
        }
        run_git(&scratch, &["checkout", "-q", "--detach", commit])?;
        let head = git_output(&scratch, &["rev-parse", "HEAD"])?;
        if head.trim() != commit {
            return Err(Error::new(
                codes::FETCH_004,
                format!(
                    "git source '{}' checked out {} but the recipe pins {}",
                    source.name,
                    head.trim(),
                    commit
                ),
            ));
        }
        build_git_tar(&scratch, &source.name, temp_dir)
    })();
    std::fs::remove_dir_all(&scratch).ok();
    result
}

fn run_git(directory: &Path, args: &[&str]) -> Result<()> {
    git_output(directory, args).map(|_| ())
}

fn git_output(directory: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot run git: {error}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(Error::new(
            codes::FETCH_003,
            format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ))
    }
}

fn build_git_tar(root: &Path, prefix: &str, temp_dir: &Path) -> Result<tempfile::TempPath> {
    use std::os::unix::fs::PermissionsExt;
    if prefix.is_empty() || prefix.contains('/') || prefix == "." || prefix == ".." {
        return Err(Error::new(
            codes::FETCH_002,
            format!("git source name '{prefix}' cannot be used as a directory prefix"),
        ));
    }
    let mut entries = Vec::new();
    collect_git_entries(root, root, &mut entries)?;
    entries.sort();

    let output = tempfile::NamedTempFile::new_in(temp_dir).map_err(|error| {
        Error::new(
            codes::FETCH_003,
            format!("cannot create a staging file for git source '{prefix}': {error}"),
        )
    })?;
    let encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(output, flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    for relative in entries {
        let path = root.join(&relative);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            Error::new(
                codes::FETCH_003,
                format!("cannot stat {}: {error}", path.display()),
            )
        })?;
        let name = format!("{prefix}/{relative}");
        let mut header = tar::Header::new_gnu();
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mode(metadata.permissions().mode() & 0o777);
        if metadata.is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_cksum();
            tar.append_data(&mut header, &name, std::io::empty())
                .map_err(|error| {
                    Error::new(codes::FETCH_003, format!("cannot archive {name}: {error}"))
                })?;
        } else if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path).map_err(|error| {
                Error::new(
                    codes::FETCH_003,
                    format!("cannot read symlink {}: {error}", path.display()),
                )
            })?;
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_link_name(&target).map_err(|error| {
                Error::new(codes::FETCH_003, format!("invalid symlink target: {error}"))
            })?;
            header.set_cksum();
            tar.append_data(&mut header, &name, std::io::empty())
                .map_err(|error| {
                    Error::new(codes::FETCH_003, format!("cannot archive {name}: {error}"))
                })?;
        } else if metadata.is_file() {
            let mut file = std::fs::File::open(&path).map_err(|error| {
                Error::new(
                    codes::FETCH_003,
                    format!("cannot read {}: {error}", path.display()),
                )
            })?;
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(metadata.len());
            header.set_cksum();
            tar.append_data(&mut header, &name, &mut file)
                .map_err(|error| {
                    Error::new(codes::FETCH_003, format!("cannot archive {name}: {error}"))
                })?;
        }
    }
    let encoder = tar
        .into_inner()
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot finish tar: {error}")))?;
    let output = encoder
        .finish()
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot finish gzip: {error}")))?;
    Ok(output.into_temp_path())
}

fn collect_git_entries(root: &Path, directory: &Path, entries: &mut Vec<String>) -> Result<()> {
    let children = rpmx_base::io::sorted_entries(directory, codes::FETCH_003)?;
    for child in children {
        let path = child.path();
        if child.file_name() == ".git" {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| Error::new(codes::FETCH_003, format!("cannot relativize: {error}")))?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        entries.push(relative);
        let file_type = child.file_type().map_err(|error| {
            Error::new(
                codes::FETCH_003,
                format!("cannot stat {}: {error}", path.display()),
            )
        })?;
        if file_type.is_dir() {
            collect_git_entries(root, &path, entries)?;
        }
    }
    Ok(())
}
