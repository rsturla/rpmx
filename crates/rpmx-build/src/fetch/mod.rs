use std::path::{Path, PathBuf};
use std::time::Duration;

use rpmx_base::error::{codes, Error, Result};
use rpmx_base::hash::{sha256_file, sha512_file};
use rpmx_language::model::{Extract, Locator, Recipe, Source};

mod git;
mod http;

#[derive(Debug, Clone)]
pub struct FetchedSource {
    pub name: String,
    pub digest: String,
    pub path: PathBuf,
    /// How to unpack the cached payload into the workspace.
    pub extract: Option<Extract>,
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

    let temp_dir = options.cache.join("tmp");
    std::fs::create_dir_all(&temp_dir).map_err(|error| {
        Error::new(
            codes::FETCH_005,
            format!("cannot create {}: {error}", temp_dir.display()),
        )
    })?;

    let mut fetched = Vec::with_capacity(recipe.sources.len());
    for source in &recipe.sources {
        let hex = digest_hex(source)?;
        let path = rpmx_store::cas::path(options.cache, &source.digest).map_err(|error| {
            Error::new(
                codes::FETCH_001,
                format!(
                    "source '{}' cannot use the CAS: {}",
                    source.name, error.message
                ),
            )
        })?;
        if path.exists() {
            fetched.push(FetchedSource {
                name: source.name.clone(),
                digest: source.digest.clone(),
                path,
                extract: extract_spec(source),
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

        let staged = if let Some(proxy) = options.proxy {
            match http::proxy_get(&agent, proxy, hex, &temp_dir) {
                Ok(Some(staged)) => staged,
                Ok(None) => {
                    fetch_from_locators(&agent, source, options.base_dir, options.cache, &temp_dir)?
                }
                Err(error) => return Err(error),
            }
        } else {
            fetch_from_locators(&agent, source, options.base_dir, options.cache, &temp_dir)?
        };

        verify_payload(source, &staged, "")?;
        rpmx_store::cas::store_file(options.cache, staged.as_ref(), &source.digest).map_err(
            |error| {
                Error::new(
                    codes::FETCH_005,
                    format!("cannot publish source '{}': {}", source.name, error.message),
                )
            },
        )?;
        fetched.push(FetchedSource {
            name: source.name.clone(),
            digest: source.digest.clone(),
            path,
            extract: extract_spec(source),
        });
    }
    Ok(fetched)
}

fn verify_payload(source: &Source, staged: &tempfile::TempPath, context: &str) -> Result<()> {
    let actual = match source
        .digest
        .split_once(':')
        .map(|(algorithm, _)| algorithm)
    {
        Some("sha256") => sha256_file(staged.as_ref()),
        _ => sha512_file(staged.as_ref()),
    }
    .map_err(|error| {
        Error::new(
            codes::FETCH_005,
            format!("cannot hash source '{}': {error}", source.name),
        )
    })?;
    if actual == source.digest {
        Ok(())
    } else {
        Err(Error::new(
            codes::FETCH_004,
            format!(
                "source '{}' digest mismatch{context}: expected {}, got {actual}",
                source.name, source.digest
            ),
        ))
    }
}

fn digest_hex(source: &Source) -> Result<&str> {
    let (algorithm, hex) = source.digest.split_once(':').ok_or_else(|| {
        Error::new(
            codes::FETCH_001,
            format!("source '{}' has no digest algorithm prefix", source.name),
        )
    })?;
    let expected = match algorithm {
        "sha512" => 128,
        "sha256" => 64,
        other => {
            return Err(Error::new(
                codes::FETCH_001,
                format!(
                    "source '{}' uses digest algorithm '{other}'; use sha512 or sha256",
                    source.name
                ),
            ))
        }
    };
    if hex.len() != expected || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::new(
            codes::FETCH_001,
            format!(
                "source '{}' has a malformed {algorithm} digest",
                source.name
            ),
        ));
    }
    Ok(hex)
}

fn extract_spec(source: &Source) -> Option<Extract> {
    if let Some(extract) = &source.extract {
        return Some(extract.clone());
    }
    let archive = source.kind == "git"
        || source.kind == "archive"
        || [".tar.gz", ".tgz", ".tar.xz", ".tar.zst"]
            .iter()
            .any(|suffix| source.name.ends_with(suffix));
    archive.then(|| Extract {
        format: Some("auto".to_string()),
        strip_components: Some(1),
        destination: Some(".".to_string()),
    })
}

fn fetch_from_locators(
    agent: &ureq::Agent,
    source: &Source,
    base_dir: &Path,
    cache: &Path,
    temp_dir: &Path,
) -> Result<tempfile::TempPath> {
    if source.locators.is_empty() {
        return Err(Error::new(
            codes::FETCH_002,
            format!("source '{}' has no locators", source.name),
        ));
    }
    let mut last_error = None;
    for locator in &source.locators {
        match fetch_locator(agent, source, locator, base_dir, cache, temp_dir) {
            Ok(staged) => {
                match verify_payload(source, &staged, &format!(" from {} locator", locator.kind)) {
                    Ok(()) => return Ok(staged),
                    Err(error) => last_error = Some(error),
                }
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
    temp_dir: &Path,
) -> Result<tempfile::TempPath> {
    match locator.kind.as_str() {
        "git" => git::git_archive(source, locator, cache, temp_dir),
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
            let metadata = std::fs::metadata(&path).map_err(|error| {
                Error::new(
                    codes::FETCH_005,
                    format!("cannot stat {}: {error}", path.display()),
                )
            })?;
            if !metadata.is_file() {
                return Err(Error::new(
                    codes::FETCH_005,
                    format!("file locator {} is not a regular file", path.display()),
                ));
            }
            let file = std::fs::File::open(&path).map_err(|error| {
                Error::new(
                    codes::FETCH_005,
                    format!("cannot open {}: {error}", path.display()),
                )
            })?;
            http::stage_reader(file, temp_dir, &format!("file {}", path.display()))
        }
        "https" => {
            let url = locator.url.as_deref().ok_or_else(|| {
                Error::new(
                    codes::FETCH_002,
                    format!("https locator for '{}' has no url", source.name),
                )
            })?;
            http::get(agent, url, temp_dir)
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
            http::get(agent, &url, temp_dir)
        }
        other => Err(Error::new(
            codes::FETCH_007,
            format!("locator kind '{other}' is not supported yet"),
        )),
    }
}
