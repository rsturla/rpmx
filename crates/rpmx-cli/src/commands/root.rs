use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::pkgmanifest::{self, RpmPackageManifest};
use rpmx_core::profile::load_profile;
use rpmx_core::root::{
    assemble as assemble_root, load_lock, resolve_profile, write_lock, AssembleOptions, RootLock,
};
use rpmx_core::tree::{digest_root, tree_digest};

use rpmx_core::Error;

use crate::commands::{
    RootAssembleArgs, RootDigestArgs, RootExportArgs, RootImportArgs, RootResolveArgs,
};

pub fn resolve(args: RootResolveArgs) -> Result<(), Error> {
    let output = args.output.as_deref();
    let profile = load_profile(&args.profile)?;
    let fallback = args
        .profile
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("root");
    let name = profile.name(fallback);
    let cache = cache_dir(args.cache.cache.as_deref())?;
    let (work, temporary) = scratch(args.work.as_deref())?;

    let result = resolve_profile(&profile, &name, &work, &cache).and_then(|lock| {
        emit_lock(&lock, output)?;
        if args.output.is_some() {
            if rpmx_core::output::json() {
                print_json("root-resolve", &lock)?;
            } else {
                println!(
                    "{} {} {} packages closure={}",
                    lock.profile,
                    lock.archs.join(","),
                    package_count(&lock),
                    lock.closure_digest
                );
            }
        }
        Ok(())
    });

    cleanup(&work, temporary, args.keep);
    result
}

pub fn assemble(args: RootAssembleArgs) -> Result<(), Error> {
    let mut lock = load_lock(&args.lock)?;
    let output = args.output.as_deref();

    let root = match args.root.as_deref() {
        Some(path) => path.to_path_buf(),
        None => output
            .and_then(Path::parent)
            .map(|parent| parent.join("root"))
            .ok_or_else(|| {
                Error::new(
                    codes::ROOT_012,
                    "pass --root when writing the assembled lock to stdout",
                )
            })?,
    };
    let root = absolute(&root)?;
    let cache = cache_dir(args.cache.cache.as_deref())?;
    let (work, temporary) = scratch(args.work.as_deref())?;

    let result = assemble_root(
        &lock,
        AssembleOptions {
            cache: &cache,
            work: &work,
            root: &root,
        },
    )
    .and_then(|root| {
        let digest = digest_root(&root)?;
        lock.root_digest = Some(digest.clone());
        emit_lock(&lock, output)?;
        if output.is_some() {
            if rpmx_core::output::json() {
                print_json("root-assemble", &lock)?;
            } else {
                println!(
                    "{} {} {} packages root={} root_digest={}",
                    lock.profile,
                    lock.archs.join(","),
                    package_count(&lock),
                    root.display(),
                    digest
                );
            }
        }
        Ok(())
    });

    cleanup(&work, temporary, args.keep);
    result
}

pub fn digest(args: RootDigestArgs) -> Result<(), Error> {
    let digest = tree_digest(&args.root)?;
    if rpmx_core::output::json() {
        let value = crate::commands::report::success(
            "root-digest",
            serde_json::json!({ "root_digest": digest }),
        );
        println!("{value}");
    } else {
        println!("{digest}");
    }
    Ok(())
}

pub fn export(args: RootExportArgs) -> Result<(), Error> {
    let lock = load_lock(&args.lock)?;
    if lock.options.weak_deps || lock.options.docs {
        eprintln!(
            "warning: rpm-package-manifest cannot represent resolve options; \
             importing this manifest will not reproduce the root"
        );
    }
    let manifest = pkgmanifest::from_lock(&lock);
    let text = serde_yaml_ng::to_string(&manifest)
        .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?;
    match args.output.as_deref() {
        Some(path) => write_file(path, &text),
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

pub fn import(args: RootImportArgs) -> Result<(), Error> {
    let output = args.output.as_deref();
    let text = std::fs::read_to_string(&args.manifest).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot read {}: {error}", args.manifest.display()),
        )
    })?;
    let manifest: RpmPackageManifest = serde_yaml_ng::from_str(&text)
        .map_err(|error| Error::new(codes::ROOT_006, format!("invalid manifest: {error}")))?;
    let lock = pkgmanifest::to_lock(
        &manifest,
        &args.profile,
        args.arch.as_deref(),
        &args.releasever,
    )?;
    emit_lock(&lock, output)?;
    if args.output.is_none() {
        return Ok(());
    }
    println!(
        "{} {} {} packages closure={}",
        lock.profile,
        lock.archs.join(","),
        package_count(&lock),
        lock.closure_digest
    );
    Ok(())
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
        None => std::env::temp_dir().join(format!("rpmx-root-{}", std::process::id())),
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
    let path = std::fs::canonicalize(&path).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot resolve {}: {error}", path.display()),
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

fn package_count(lock: &RootLock) -> usize {
    lock.packages.values().map(Vec::len).sum()
}

fn print_json(command: &'static str, lock: &RootLock) -> Result<(), Error> {
    let value = crate::commands::report::success(
        command,
        serde_json::to_value(lock)
            .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?,
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&value)
            .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
    );
    Ok(())
}

fn emit_lock(lock: &RootLock, output: Option<&Path>) -> Result<(), Error> {
    match output {
        Some(path) => write_lock(path, lock),
        None => {
            let text = serde_yaml_ng::to_string(lock)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?;
            print!("{text}");
            Ok(())
        }
    }
}

fn write_file(path: &Path, text: &str) -> Result<(), Error> {
    std::fs::write(path, text).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}
