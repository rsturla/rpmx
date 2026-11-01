use rpmx_core::error::codes;
use rpmx_core::source::{assemble, SourceOptions};
use rpmx_core::Error;

use crate::commands::fetch::default_cache;
use crate::commands::{digest_root, load_plan, SourceArgs};

pub fn run(args: SourceArgs<'_>) -> Result<std::path::PathBuf, Error> {
    let SourceArgs {
        file,
        pipeline_dir,
        policy,
        cache,
        proxy,
        offline,
        output,
        arch,
        runner,
        source_date_epoch,
        jobs,
        step_timeout,
        root_lock,
        root,
        unsafe_host_root,
        quiet,
        json,
    } = args;
    let cache = match cache {
        Some(path) => path.to_path_buf(),
        None => default_cache()?,
    };
    let proxy = proxy
        .map(str::to_string)
        .or_else(|| std::env::var("RPMX_PROXY").ok())
        .or_else(|| std::env::var("RPMX_ACQUISITION_PROXY").ok());
    let (recipe, plan) = load_plan(file, arch, pipeline_dir, false, policy)?;
    let source_date_epoch = crate::commands::source_date_epoch(&recipe, source_date_epoch);
    let work = tempfile::tempdir().map_err(|error| {
        Error::new(
            codes::SOURCE_030,
            format!("cannot create work directory: {error}"),
        )
    })?;
    let workspace = work.path().join("build");
    let stage = work.path().join("stage");
    let base_dir = file.parent().unwrap_or_else(|| std::path::Path::new("."));

    if unsafe_host_root && (root_lock.is_some() || root.is_some()) {
        return Err(Error::new(
            codes::ROOT_016,
            "--unsafe-host-root cannot be combined with --root-lock or --root",
        ));
    }
    if !unsafe_host_root && (root_lock.is_none() || root.is_none()) {
        return Err(Error::new(
            codes::ROOT_016,
            "source assembly requires --root-lock and --root; \
             pass --unsafe-host-root to use the host root",
        ));
    }

    let mut selected_lock: Option<rpmx_core::root::RootLock> = None;
    let selected_root = match (root_lock, root) {
        (Some(lock_path), Some(root_path)) => {
            let lock = rpmx_core::root::load_lock(lock_path)?;
            let expected = lock.root_digest.as_deref().ok_or_else(|| {
                Error::new(
                    codes::ROOT_016,
                    "the source root lock has no root_digest; run `rpmx root assemble` first",
                )
            })?;
            let wanted = rpmx_core::profile::normalize_arch(arch)?;
            if !lock
                .archs
                .iter()
                .any(|arch| rpmx_core::profile::normalize_arch(arch).ok() == Some(wanted))
            {
                return Err(Error::new(
                    codes::ROOT_016,
                    format!("source root lock is for {}", lock.archs.join(", ")),
                ));
            }
            let root_path = std::fs::canonicalize(root_path).map_err(|error| {
                Error::new(
                    codes::ROOT_017,
                    format!("cannot resolve {}: {error}", root_path.display()),
                )
            })?;
            let actual = digest_root(&root_path)?;
            if actual != expected {
                return Err(Error::new(
                    codes::ROOT_017,
                    format!(
                        "root digest mismatch: lock records {expected}, {} is {actual}",
                        root_path.display()
                    ),
                ));
            }
            selected_lock = Some(lock);
            Some(root_path)
        }
        (Some(_), None) => {
            return Err(Error::new(
                codes::ROOT_016,
                "--root-lock for source assembly requires --root",
            ));
        }
        (None, Some(_)) => {
            return Err(Error::new(
                codes::ROOT_016,
                "--root for source assembly requires --root-lock",
            ));
        }
        (None, None) => None,
    };

    let snapshot = assemble(SourceOptions {
        recipe: &recipe,
        plan: &plan,
        cache: &cache,
        proxy: proxy.as_deref(),
        offline,
        base_dir,
        workspace: &workspace,
        stage: &stage,
        output,
        source_date_epoch,
        jobs,
        step_timeout,
        root: selected_root.as_deref(),
        runner,
        recipe_dir: file.parent().unwrap_or_else(|| std::path::Path::new(".")),
    })?;

    if let Some(lock) = &selected_lock {
        let text = serde_yaml_ng::to_string(lock)
            .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?;
        let path = output.join("source-root.lock.json");
        std::fs::write(&path, text).map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }

    if !quiet {
        if json {
            let value = serde_json::json!({
                "ok": true,
                "snapshot": snapshot.display().to_string(),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&value)
                    .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
            );
        } else {
            println!("{}", snapshot.display());
        }
    }
    Ok(snapshot)
}
