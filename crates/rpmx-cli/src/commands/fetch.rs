use std::path::PathBuf;

use rpmx_core::error::codes;
use rpmx_core::fetch::{fetch_sources, FetchOptions};
use rpmx_core::{load_recipe, Error};

use crate::commands::FetchArgs;

pub fn run(args: FetchArgs<'_>) -> Result<(), Error> {
    let FetchArgs {
        file,
        cache,
        proxy,
        offline,
        json,
    } = args;
    let recipe = load_recipe(file)?;
    let cache = match cache {
        Some(path) => path.to_path_buf(),
        None => default_cache()?,
    };
    let base_dir = file.parent().unwrap_or_else(|| std::path::Path::new("."));
    let proxy = proxy
        .map(str::to_string)
        .or_else(|| std::env::var("RPMX_PROXY").ok())
        .or_else(|| std::env::var("RPMX_ACQUISITION_PROXY").ok());

    let fetched = fetch_sources(
        &recipe,
        FetchOptions {
            cache: &cache,
            proxy: proxy.as_deref(),
            offline,
            base_dir,
        },
    )?;

    if json {
        let value = serde_json::json!({
            "ok": true,
            "cache": cache.display().to_string(),
            "sources": fetched.iter().map(|source| serde_json::json!({
                "name": source.name,
                "digest": source.digest,
                "path": source.path.display().to_string(),
            })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
        );
    } else {
        for source in &fetched {
            println!(
                "{} {} {}",
                source.name,
                source.digest,
                source.path.display()
            );
        }
    }
    Ok(())
}

pub fn default_cache() -> Result<PathBuf, Error> {
    if let Ok(base) = std::env::var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(base).join("rpmx").join("cas"));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home).join(".cache").join("rpmx").join("cas"));
    }
    Err(Error::new(
        codes::FETCH_005,
        "cannot determine cache directory; pass --cache",
    ))
}
