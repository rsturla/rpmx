use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use rpmx_core::error::codes;
use rpmx_core::{cas, load_recipe, root::load_lock, Error};

use crate::commands::fetch::default_cache;
use crate::commands::GcArgs;

pub fn gc(args: GcArgs) -> Result<(), Error> {
    let cache = match &args.cache.cache {
        Some(path) => path.clone(),
        None => default_cache()?,
    };
    let keep = referenced_digests(&args.lock, &args.recipe)?;
    if keep.is_empty() {
        return Err(Error::new(
            codes::CACHE_001,
            "refusing to garbage-collect without references; \
             pass --lock FILE and/or --recipe FILE",
        ));
    }

    let report = cas::gc(
        &cache,
        cas::GcOptions {
            keep: &keep,
            older_than: Duration::from_secs(args.older_than_hours.saturating_mul(3600)),
            dry_run: args.dry_run,
            now: std::time::SystemTime::now(),
        },
    )?;

    if rpmx_core::output::json() {
        let value = crate::commands::report::success(
            "cache",
            serde_json::json!({
                "cache": cache.display().to_string(),
                "protected": keep.len(),
                "kept": report.kept,
                "removed": report.removed,
                "bytes": report.bytes,
                "dry_run": args.dry_run,
                "blobs": report.blobs,
            }),
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_003, error.to_string()))?
        );
    } else if args.dry_run {
        println!(
            "would remove {} blob(s), {} bytes; {} protected",
            report.removed,
            report.bytes,
            keep.len()
        );
    } else {
        println!(
            "removed {} blob(s), {} bytes; {} protected",
            report.removed,
            report.bytes,
            keep.len()
        );
    }
    Ok(())
}

fn referenced_digests(locks: &[PathBuf], recipes: &[PathBuf]) -> Result<BTreeSet<String>, Error> {
    let mut keep = BTreeSet::new();
    for path in locks {
        let lock = load_lock(path)?;
        for packages in lock.packages.values() {
            for package in packages {
                keep.insert(package.checksum.clone());
            }
        }
    }
    for path in recipes {
        let recipe = load_recipe(path)?;
        for source in &recipe.sources {
            keep.insert(source.digest.clone());
        }
        for tool in &recipe.source_assembly.tools {
            keep.insert(tool.digest.clone());
        }
    }
    Ok(keep)
}
