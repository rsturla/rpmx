use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{codes, Error, Result};
use crate::hash::sha512_hex;
use crate::model::{Locator, Recipe, Source};

mod git;
mod http;

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
        let path = crate::cas::path(options.cache, &source.digest).map_err(|error| {
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
            match http::proxy_get(&agent, proxy, hex) {
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
        crate::cas::store(options.cache, &bytes).map_err(|error| {
            Error::new(
                codes::FETCH_005,
                format!("cannot publish source '{}': {}", source.name, error.message),
            )
        })?;
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
        "git" => git::git_archive(source, locator, cache),
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
            http::read_body(response.body_mut())
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
            http::read_body(response.body_mut())
        }
        other => Err(Error::new(
            codes::FETCH_007,
            format!("locator kind '{other}' is not supported yet"),
        )),
    }
}
