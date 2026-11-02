//! Inspect, resolve and assemble runner-owned assets independently.
//!
//! The QEMU transport appliance is the only asset kind today.

use std::path::{Path, PathBuf};

use rpmx_core::appliance;
use rpmx_core::error::codes;
use rpmx_core::profile::load_profile;
use rpmx_core::root::{load_lock, ApplianceLock, AssembleOptions};
use rpmx_core::Error;

use crate::commands::{ApplianceAssembleArgs, ApplianceInspectArgs, ApplianceResolveArgs};

pub fn resolve(args: ApplianceResolveArgs) -> Result<(), Error> {
    let profile = load_profile(&args.profile)?;
    let cache = cache_dir(args.cache.cache.as_deref())?;
    let fallback = args
        .profile
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("appliance");
    let name = profile.name(fallback);
    let (work, temporary) = scratch(args.work.as_deref())?;
    let result = appliance::resolve(&profile, &name, &work, &cache).and_then(|lock| {
        let text = serde_yaml_ng::to_string(&lock)
            .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
        match args.output.as_deref() {
            Some(path) => std::fs::write(path, text).map_err(|error| {
                Error::new(
                    codes::ROOT_004,
                    format!("cannot write {}: {error}", path.display()),
                )
            })?,
            None => print!("{text}"),
        }
        if args.output.is_some() {
            eprintln!(
                "appliance {} schema={} packages={} identity={}",
                lock.profile,
                lock.schema,
                count_packages(&lock),
                lock.identity()
            );
        }
        Ok(())
    });
    cleanup(&work, temporary, args.keep);
    result
}

pub fn assemble(args: ApplianceAssembleArgs) -> Result<(), Error> {
    let text = std::fs::read_to_string(&args.lock).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot read {}: {error}", args.lock.display()),
        )
    })?;
    let lock: ApplianceLock = serde_yaml_ng::from_str(&text)
        .map_err(|error| Error::new(codes::ROOT_021, format!("invalid appliance lock: {error}")))?;
    lock.validate()?;
    if args.offline && args.root.is_some() {
        return Err(Error::new(
            codes::ROOT_020,
            "--offline cannot be combined with --root; assemble from the cache or omit --offline",
        ));
    }
    let cache = cache_dir(args.cache.cache.as_deref())?;
    let root = match args.root.as_deref() {
        Some(path) => {
            let path = absolute(path)?;
            let work = tempfile::Builder::new()
                .prefix("rpmx-appliance-")
                .tempdir()
                .map_err(|error| {
                    Error::new(
                        codes::ROOT_004,
                        format!("cannot create an appliance work directory: {error}"),
                    )
                })?;
            appliance::assemble_root(
                &lock,
                AssembleOptions {
                    cache: &cache,
                    work: work.path(),
                    root: &path,
                },
            )?
        }
        None => appliance::ensure_cached(&lock, &cache, args.offline)?,
    };
    println!(
        "runner_asset_root={} identity={}",
        root.display(),
        lock.identity()
    );
    Ok(())
}

pub fn inspect(args: ApplianceInspectArgs) -> Result<(), Error> {
    let lock = match (args.root_lock.as_deref(), args.asset_lock.as_deref()) {
        (Some(path), None) => load_lock(path)?
            .qemu_appliance()
            .cloned()
            .ok_or_else(|| Error::new(codes::ROOT_020, "the root lock has no QEMU appliance"))?,
        (None, Some(path)) => {
            let text = std::fs::read_to_string(path).map_err(|error| {
                Error::new(
                    codes::ROOT_004,
                    format!("cannot read {}: {error}", path.display()),
                )
            })?;
            let lock: ApplianceLock = serde_yaml_ng::from_str(&text).map_err(|error| {
                Error::new(codes::ROOT_021, format!("invalid appliance lock: {error}"))
            })?;
            lock
        }
        _ => {
            return Err(Error::new(
                codes::ROOT_020,
                "pass exactly one of --root-lock or --asset-lock",
            ))
        }
    };
    lock.validate()?;
    let packages = count_packages(&lock);
    if rpmx_core::output::json() {
        println!(
            "{}",
            serde_json::json!({
                "profile": lock.profile,
                "schema": lock.schema,
                "releasever": lock.releasever,
                "archs": lock.archs,
                "closure_digest": lock.closure_digest,
                "identity": lock.identity(),
                "packages": packages,
            })
        );
    } else {
        println!("profile={}", lock.profile);
        println!("schema={}", lock.schema);
        println!("releasever={}", lock.releasever);
        println!("archs={}", lock.archs.join(","));
        println!("packages={packages}");
        println!("closure_digest={}", lock.closure_digest);
        println!("identity={}", lock.identity());
    }
    Ok(())
}

fn count_packages(lock: &ApplianceLock) -> usize {
    lock.packages.values().map(Vec::len).sum()
}

fn cache_dir(explicit: Option<&Path>) -> Result<PathBuf, Error> {
    match explicit {
        Some(path) => Ok(path.to_path_buf()),
        None => crate::commands::fetch::default_cache(),
    }
}

fn scratch(explicit: Option<&Path>) -> Result<(PathBuf, bool), Error> {
    let temporary = explicit.is_none();
    let path = match explicit {
        Some(path) => path.to_path_buf(),
        None => std::env::temp_dir().join(format!("rpmx-appliance-{}", std::process::id())),
    };
    if temporary {
        let _ = std::fs::remove_dir_all(&path);
    }
    std::fs::create_dir_all(&path).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot create {}: {error}", path.display()),
        )
    })?;
    Ok((path, temporary))
}

fn cleanup(work: &Path, temporary: bool, keep: bool) {
    if temporary && !keep {
        let _ = std::fs::remove_dir_all(work);
    } else if temporary {
        eprintln!("scratch kept at {}", work.display());
    }
}

fn absolute(path: &Path) -> Result<PathBuf, Error> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| {
                Error::new(
                    codes::ROOT_004,
                    format!("cannot resolve {}: {error}", path.display()),
                )
            })
    }
}
