use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{codes, Error, Result};
use crate::hash::sha512_hex;
use crate::model::{Locator, Recipe, Source};

#[derive(Debug, Clone)]
pub struct FetchedSource {
    pub name: String,
    pub digest: String,
    pub path: PathBuf,
    /// Whether the cached payload is an archive to unpack into the workspace.
    pub archive: bool,
}

pub struct FetchOptions<'a> {
    pub cache: &'a Path,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub base_dir: &'a Path,
}

pub fn fetch_sources(recipe: &Recipe, options: FetchOptions<'_>) -> Result<Vec<FetchedSource>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();

    let mut fetched = Vec::with_capacity(recipe.sources.len());
    for source in &recipe.sources {
        let hex = digest_hex(source)?;
        let path = options.cache.join("sha512").join(hex);
        if path.exists() {
            fetched.push(FetchedSource {
                name: source.name.clone(),
                digest: source.digest.clone(),
                path,
                archive: is_archive(source),
            });
            continue;
        }
        if options.offline {
            return Err(Error::new(
                codes::FETCH_006,
                format!(
                    "source '{}' is not in the cache and --offline was set",
                    source.name
                ),
            ));
        }

        let bytes = if let Some(proxy) = options.proxy {
            match proxy_get(&agent, proxy, hex) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => fetch_from_locators(&agent, source, options.base_dir, options.cache)?,
                Err(error) => return Err(error),
            }
        } else {
            fetch_from_locators(&agent, source, options.base_dir, options.cache)?
        };

        let actual = sha512_hex(&bytes);
        if actual != source.digest {
            return Err(Error::new(
                codes::FETCH_004,
                format!(
                    "source '{}' digest mismatch: expected {}, got {}",
                    source.name, source.digest, actual
                ),
            ));
        }
        write_cas(&path, &bytes)?;
        fetched.push(FetchedSource {
            name: source.name.clone(),
            digest: source.digest.clone(),
            path,
            archive: is_archive(source),
        });
    }
    Ok(fetched)
}

fn digest_hex(source: &Source) -> Result<&str> {
    let hex = source.digest.strip_prefix("sha512:").ok_or_else(|| {
        Error::new(
            codes::FETCH_001,
            format!("source '{}' does not use a sha512 digest", source.name),
        )
    })?;
    if hex.len() != 128 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new(
            codes::FETCH_001,
            format!("source '{}' has a malformed sha512 digest", source.name),
        ));
    }
    Ok(hex)
}

fn proxy_get(agent: &ureq::Agent, proxy: &str, hex: &str) -> Result<Option<Vec<u8>>> {
    let url = format!("{}/v1/blobs/sha512/{hex}", proxy.trim_end_matches('/'));
    match agent.get(&url).call() {
        Ok(mut response) => Ok(Some(read_body(response.body_mut())?)),
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(error) => Err(Error::new(
            codes::FETCH_003,
            format!("proxy request failed: {error}"),
        )),
    }
}

fn is_archive(source: &Source) -> bool {
    source.kind == "git" || source.name.ends_with(".tar.gz") || source.name.ends_with(".tgz")
}

fn fetch_from_locators(
    agent: &ureq::Agent,
    source: &Source,
    base_dir: &Path,
    cache: &Path,
) -> Result<Vec<u8>> {
    if source.locators.is_empty() {
        return Err(Error::new(
            codes::FETCH_002,
            format!("source '{}' has no locators", source.name),
        ));
    }
    let mut last_error = None;
    for locator in &source.locators {
        match fetch_locator(agent, source, locator, base_dir, cache) {
            Ok(bytes) => {
                let actual = sha512_hex(&bytes);
                if actual == source.digest {
                    return Ok(bytes);
                }
                last_error = Some(Error::new(
                    codes::FETCH_004,
                    format!(
                        "source '{}' digest mismatch from {} locator: expected {}, got {}",
                        source.name, locator.kind, source.digest, actual
                    ),
                ));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        Error::new(
            codes::FETCH_002,
            format!("source '{}' could not be fetched", source.name),
        )
    }))
}

fn fetch_locator(
    agent: &ureq::Agent,
    source: &Source,
    locator: &Locator,
    base_dir: &Path,
    cache: &Path,
) -> Result<Vec<u8>> {
    match locator.kind.as_str() {
        "git" => git_archive(source, locator, cache),
        "file" => {
            let raw = locator
                .path
                .as_deref()
                .or(locator.url.as_deref())
                .ok_or_else(|| {
                    Error::new(
                        codes::FETCH_002,
                        format!("file locator for '{}' has no path", source.name),
                    )
                })?;
            let raw = raw.strip_prefix("file://").unwrap_or(raw);
            let path = if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                base_dir.join(raw)
            };
            std::fs::read(&path).map_err(|error| {
                Error::new(
                    codes::FETCH_005,
                    format!("cannot read {}: {error}", path.display()),
                )
            })
        }
        "https" => {
            let url = locator.url.as_deref().ok_or_else(|| {
                Error::new(
                    codes::FETCH_002,
                    format!("https locator for '{}' has no url", source.name),
                )
            })?;
            let mut response = agent.get(url).call().map_err(|error| {
                Error::new(codes::FETCH_003, format!("GET {url} failed: {error}"))
            })?;
            read_body(response.body_mut())
        }
        "go-module" => {
            let proxy = locator.proxy.as_deref().ok_or_else(|| {
                Error::new(
                    codes::FETCH_002,
                    format!("go-module locator for '{}' has no proxy", source.name),
                )
            })?;
            let module = source.module.as_deref().ok_or_else(|| {
                Error::new(
                    codes::FETCH_002,
                    format!("source '{}' has no module", source.name),
                )
            })?;
            let version = source.version.as_deref().ok_or_else(|| {
                Error::new(
                    codes::FETCH_002,
                    format!("source '{}' has no version", source.name),
                )
            })?;
            let url = format!(
                "{}/{}/@v/{}.zip",
                proxy.trim_end_matches('/'),
                module,
                version
            );
            let mut response = agent.get(&url).call().map_err(|error| {
                Error::new(codes::FETCH_003, format!("GET {url} failed: {error}"))
            })?;
            read_body(response.body_mut())
        }
        other => Err(Error::new(
            codes::FETCH_007,
            format!("locator kind '{other}' is not supported yet"),
        )),
    }
}

fn git_archive(source: &Source, locator: &Locator, cache: &Path) -> Result<Vec<u8>> {
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
        build_git_tar(&scratch, &source.name)
    })();
    std::fs::remove_dir_all(&scratch).ok();
    result
}

fn run_git(directory: &Path, args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot run git: {error}")))?;
    if output.status.success() {
        Ok(())
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

fn build_git_tar(root: &Path, prefix: &str) -> Result<Vec<u8>> {
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

    let encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::default());
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
            let data = std::fs::read(&path).map_err(|error| {
                Error::new(
                    codes::FETCH_003,
                    format!("cannot read {}: {error}", path.display()),
                )
            })?;
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(data.len() as u64);
            header.set_cksum();
            tar.append_data(&mut header, &name, data.as_slice())
                .map_err(|error| {
                    Error::new(codes::FETCH_003, format!("cannot archive {name}: {error}"))
                })?;
        }
    }
    let encoder = tar
        .into_inner()
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot finish tar: {error}")))?;
    encoder
        .finish()
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot finish gzip: {error}")))
}

fn collect_git_entries(root: &Path, directory: &Path, entries: &mut Vec<String>) -> Result<()> {
    let mut children = std::fs::read_dir(directory)
        .map_err(|error| {
            Error::new(
                codes::FETCH_003,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            Error::new(
                codes::FETCH_003,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?;
    children.sort_by_key(|entry| entry.file_name());
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

fn read_body(body: &mut ureq::Body) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    body.as_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot read response: {error}")))?;
    Ok(bytes)
}

fn write_cas(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            Error::new(
                codes::FETCH_005,
                format!("cannot create {}: {error}", parent.display()),
            )
        })?;
    }
    let temp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|error| {
        Error::new(
            codes::FETCH_005,
            format!("cannot write {}: {error}", temp.display()),
        )
    })?;
    std::fs::rename(&temp, path).map_err(|error| {
        Error::new(
            codes::FETCH_005,
            format!("cannot publish {}: {error}", path.display()),
        )
    })
}
