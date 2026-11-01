use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::root::RootSelection;
use rpmx_core::source::{assemble, SourceOptions};
use rpmx_core::Error;

use crate::commands::fetch::default_cache;
use crate::commands::{load_plan, resolve_policy, PolicyInput, SourceArgs};

pub fn run(args: SourceArgs, pipeline_dir: Option<&Path>) -> Result<std::path::PathBuf, Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    let cache = match &args.cache {
        Some(path) => path.clone(),
        None => default_cache()?,
    };
    let proxy = args
        .proxy
        .clone()
        .or_else(|| std::env::var("RPMX_PROXY").ok())
        .or_else(|| std::env::var("RPMX_ACQUISITION_PROXY").ok());
    let (recipe, plan) = load_plan(args.file.as_path(), &args.arch, pipeline_dir, false, policy)?;
    let source_date_epoch = crate::commands::source_date_epoch(&recipe, args.source_date_epoch);
    let work = tempfile::tempdir().map_err(|error| {
        Error::new(
            codes::SOURCE_030,
            format!("cannot create work directory: {error}"),
        )
    })?;
    let workspace = work.path().join("build");
    let stage = work.path().join("stage");
    let base_dir = args
        .file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    if args.unsafe_host_root && (args.root_lock.is_some() || args.root.is_some()) {
        return Err(Error::new(
            codes::ROOT_016,
            "--unsafe-host-root cannot be combined with --root-lock or --root",
        ));
    }
    if !args.unsafe_host_root && (args.root_lock.is_none() || args.root.is_none()) {
        return Err(Error::new(
            codes::ROOT_016,
            "source assembly requires --root-lock and --root; \
             pass --unsafe-host-root to use the host root",
        ));
    }

    let selected = match (args.root_lock.as_deref(), args.root.as_deref()) {
        (Some(lock_path), Some(root_path)) => {
            Some(RootSelection::load(lock_path, root_path, &args.arch)?)
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
    let selected_root = selected.as_ref().map(|selection| selection.path.as_path());

    let snapshot = assemble(SourceOptions {
        recipe: &recipe,
        plan: &plan,
        cache: &cache,
        proxy: proxy.as_deref(),
        offline: args.offline,
        base_dir,
        workspace: &workspace,
        stage: &stage,
        output: &args.output,
        source_date_epoch,
        jobs: args.jobs,
        step_timeout: args.step_timeout,
        root: selected_root,
        runner: &args.runner,
        recipe_dir: args
            .file
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
    })?;

    if let Some(selection) = &selected {
        let mut text = serde_json::to_string_pretty(&selection.lock)
            .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?;
        text.push('\n');
        let path = args.output.join("source-root.lock.json");
        std::fs::write(&path, text).map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }

    if !args.quiet {
        if args.json {
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
