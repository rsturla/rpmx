//! `rpmx test`: build a recipe when needed, then run its tests.
//!
//! A full build is the only way to get both the source workspace (for
//! build-stage tests) and the RPMs (for installed-stage tests) in the same
//! run, so this command drives the shared build pipeline with tests enabled.

use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{
    load_plan, pipeline::BuildPipeline, report, resolve_policy, OptionalSigningArgs, PlanPolicy,
    PolicyInput, TestArgs,
};

pub fn run(args: TestArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy: PlanPolicy<'_> = resolved.plan_policy();

    if args.install_dir.is_some() {
        return Err(Error::new(
            codes::TEST_004,
            "--install-dir is not implemented yet; run `rpmx build` first and then `rpmx test` without it",
        ));
    }

    let (recipe, plan) = load_plan(&args.file, &args.arch, pipeline_dir, false, policy)?;
    if args.selected.is_empty() && plan.cells.iter().all(|cell| cell.tests.is_empty()) {
        return Err(Error::new(
            codes::TEST_006,
            "this recipe declares no tests; add a tests: block",
        ));
    }
    let automatic = if args.root_lock.is_none() && args.root.is_none() {
        crate::commands::autoroot::ensure(
            &recipe,
            &plan,
            &args.file,
            args.policy.profile.as_deref(),
            args.cache.as_deref(),
            args.offline,
            false,
            &args.arch,
        )?
    } else {
        None
    };
    let root_lock = args
        .root_lock
        .clone()
        .or_else(|| automatic.as_ref().map(|root| root.lock.clone()));
    let root = args
        .root
        .clone()
        .or_else(|| automatic.as_ref().map(|root| root.root.clone()));
    if root_lock.is_none() || root.is_none() {
        return Err(Error::new(
            codes::ROOT_016,
            "tests need a build root; set build.root, pass --profile, or pass --root-lock and --root",
        ));
    }
    if let Some(root) = &automatic {
        eprintln!(
            "root {} x86_64 {} packages {} ({})",
            root.profile,
            root.packages,
            root.document.root_digest.as_deref().unwrap_or("-"),
            if root.cached { "cache" } else { "resolved" }
        );
    }

    let output = crate::commands::default_output(&args.arch);
    let signing = OptionalSigningArgs::default();
    let pipeline = BuildPipeline {
        file: &args.file,
        output: &output,
        arch: &args.arch,
        runner: &args.runner,
        policy,
        profile: args.policy.profile.as_deref(),
        cache: args.cache.as_deref(),
        proxy: None,
        offline: args.offline,
        root_lock: root_lock.as_deref(),
        root: root.as_deref(),
        unsafe_host_root: false,
        source_date_epoch: None,
        jobs: args.jobs,
        step_timeout: std::num::NonZeroU64::new(3600).expect("timeout"),
        run_tests: true,
        test_filter: &args.selected,
        junit: args.junit.as_deref(),
        no_lint: false,
        sbom: None,
        test_root_lock: None,
        signing: &signing,
        pipeline_dir,
    };
    let cells: Vec<_> = plan.cells.iter().collect();
    let built = pipeline.execute(&cells)?;
    report::print(
        args.json,
        "test",
        &[report::ArtifactGroup {
            kind: "rpm",
            paths: &built.rpms,
        }],
        None,
    )?;
    Ok(())
}
