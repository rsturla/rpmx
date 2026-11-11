//! The reviewable package lock.
//!
//! A lock records what the recipe resolved to: source digests, step digests,
//! the expanded cells, policy, and the root requirement. `build --locked`
//! refuses to run when any of those changed since the lock was written.

use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{fetch::default_cache, load_plan, resolve_policy, LockArgs, PolicyInput};

pub use rpmx_build::package_lock::*;

pub fn run(args: LockArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy = resolved.plan_policy();
    let declared = rpmx_core::load_recipe_bundle(&args.file)?;
    let arch = match &args.arch {
        Some(arch) => arch.clone(),
        None => rpmx_root::recipe::declared_arch(
            &declared.recipe,
            &declared.root,
            args.policy.profile.as_deref(),
        )?
        .unwrap_or_else(|| rpmx_core::arch::native_arch().to_string()),
    };
    let (bundle, plan) = load_plan(&args.file, &arch, pipeline_dir, false, true, policy)?;
    let resolved_source_date_epoch = if bundle.recipe.package.source_date_epoch.is_some() {
        None
    } else {
        let cache = default_cache()?;
        let proxy = std::env::var("RPMX_PROXY")
            .or_else(|_| std::env::var("RPMX_ACQUISITION_PROXY"))
            .ok();
        let fetched = rpmx_core::fetch::fetch_sources(
            &bundle.recipe,
            rpmx_core::fetch::FetchOptions {
                cache: &cache,
                proxy: proxy.as_deref(),
                offline: false,
                base_dir: args.file.parent().unwrap_or_else(|| Path::new(".")),
            },
        )?;
        Some(rpmx_core::source::resolve_source_date_epoch(
            &bundle.recipe,
            &fetched,
            None,
            bundle.source_date_epoch_default,
        ))
    };
    let lock = build(
        &bundle,
        &plan,
        policy,
        args.policy.profile.as_deref(),
        &arch,
        resolved_source_date_epoch,
        args.root_lock
            .as_deref()
            .map(rpmx_core::root::load_lock)
            .transpose()?
            .as_ref(),
        None,
        Some(&default_cache()?),
    )?;
    let path = args
        .output
        .clone()
        .unwrap_or_else(|| default_path(&args.file));

    let unpinned = lock
        .root
        .as_ref()
        .is_some_and(|root| root.root_digest.is_none())
        || lock
            .sysroot
            .as_ref()
            .is_some_and(|root| root.root_digest.is_none());
    if unpinned {
        rpmx_core::output::warning(
            "lock",
            "a root is not assembled yet, so its digest is not pinned; \
             run a build and `rpmx lock` again to pin it",
        );
    }
    if args.check {
        let mut existing = read(&path)?;
        adopt_unpinned(&mut existing, &lock);
        if existing != lock {
            return Err(Error::new(
                codes::LOCK_002,
                format!(
                    "{} is out of date ({}); run `rpmx lock` to update it",
                    path.display(),
                    differences(&existing, &lock)
                ),
            )
            .at(path.display().to_string()));
        }
        println!("lock is up to date: {}", path.display());
        return Ok(());
    }

    if args.output.as_deref() == Some(Path::new("-")) {
        println!(
            "{}",
            serde_json::to_string_pretty(&lock)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
        );
    } else {
        write(&path, &lock)?;
        println!("{}", path.display());
    }
    Ok(())
}
