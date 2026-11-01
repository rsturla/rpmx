//! `rpmx test`: build a recipe when needed, then run its tests.
//!
//! A full build is the only way to get both the source workspace (for
//! build-stage tests) and the RPMs (for installed-stage tests) in the same
//! run, so this command drives the shared build pipeline with tests enabled.

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::sandbox::{Sandbox, SandboxConfig};
use rpmx_core::test_root::{prepare as prepare_test_root, TestRootOptions};
use rpmx_core::test_runner::{run_planned_tests, TestSandboxConfig};
use rpmx_core::Error;

use crate::commands::{
    load_plan, pipeline::BuildPipeline, report, resolve_policy, OptionalSigningArgs, PlanPolicy,
    PolicyInput, TestArgs, WorkDir,
};

pub fn run(args: TestArgs, pipeline_dir: Option<&Path>) -> Result<(), Error> {
    if let Some(path) = args.junit.as_deref() {
        let _ = std::fs::remove_file(path);
    }
    let resolved = resolve_policy(PolicyInput {
        profile: args.policy.profile.as_deref(),
        dist_tag: args.policy.dist_tag.as_deref(),
        build_release: args.policy.build_release,
    })?;
    let policy: PlanPolicy<'_> = resolved.plan_policy();

    let (recipe, plan) = load_plan(&args.file, &args.arch, pipeline_dir, false, policy)?;
    if !args.selected.is_empty() {
        let known: std::collections::BTreeSet<_> = plan
            .cells
            .iter()
            .flat_map(|cell| cell.tests.iter().map(|test| test.name.as_str()))
            .collect();
        for name in &args.selected {
            if !known.contains(name.as_str()) {
                return Err(Error::new(
                    codes::TEST_004,
                    format!("no test named '{name}' in this recipe"),
                ));
            }
        }
    }
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
    if root_lock.is_none() || (args.install_dir.is_none() && root.is_none()) {
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
    if let Some(directory) = args.install_dir.as_deref() {
        return run_installed(
            &args,
            &plan,
            root_lock.as_deref().expect("checked above"),
            directory,
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

fn run_installed(
    args: &TestArgs,
    plan: &rpmx_core::Plan,
    root_lock_path: &Path,
    directory: &Path,
) -> Result<(), Error> {
    let lock = rpmx_core::root::load_lock(root_lock_path)?;
    let rpms = rpms_in(directory)?;
    let mut selected = Vec::new();
    let mut tools = std::collections::BTreeSet::new();
    for cell in &plan.cells {
        for test in &cell.tests {
            let wanted = if args.selected.is_empty() {
                test.stage == "installed"
            } else {
                args.selected.contains(&test.name)
            };
            if !wanted {
                continue;
            }
            if test.stage != "installed" {
                return Err(Error::new(
                    codes::TEST_004,
                    format!(
                        "test '{}' has stage '{}'; --install-dir can only run installed-stage tests",
                        test.name, test.stage
                    ),
                ));
            }
            selected.push(test.clone());
            tools.extend(cell.test_tools.iter().cloned());
        }
    }
    if selected.is_empty() {
        return Err(Error::new(
            codes::TEST_004,
            "no installed-stage test matches; --install-dir tests existing RPMs",
        ));
    }
    let runner = args.runner.parse::<rpmx_core::runner::RunnerKind>()?;
    let host = rpmx_core::arch::Arch::current()?;
    let target = rpmx_core::arch::Arch::parse(&args.arch)?;
    let cache = match args.cache.as_deref() {
        Some(path) => path.to_path_buf(),
        None => crate::commands::fetch::default_cache()?,
    };
    let work = WorkDir::new()?;
    let workspace = work.path().join("build");
    let stage = work.path().join("stage");
    std::fs::create_dir_all(&workspace).map_err(|error| {
        Error::new(
            codes::TEST_003,
            format!("cannot create {}: {error}", workspace.display()),
        )
    })?;
    std::fs::create_dir_all(&stage).map_err(|error| {
        Error::new(
            codes::TEST_003,
            format!("cannot create {}: {error}", stage.display()),
        )
    })?;
    let tools: Vec<String> = tools.into_iter().collect();
    let prepared = prepare_test_root(TestRootOptions {
        lock: &lock,
        pinned: None,
        tools: &tools,
        rpms: &rpms,
        image: lock.builder.as_ref().map(|builder| builder.image.as_str()),
        cache: &cache,
        work: work.path(),
        runner,
    })?;
    let source_date_epoch = 0;
    let timeout = NonZeroU64::new(3600).expect("timeout");
    let sandbox = Sandbox::new(SandboxConfig {
        runner,
        host,
        target,
        workspace: &workspace,
        stage: &stage,
        root: Some(&prepared.path),
        image: None,
        source_date_epoch,
        jobs: args.jobs,
        timeout,
        debuginfo: false,
    })?;
    let settings = TestSandboxConfig {
        workspace: &workspace,
        stage: &stage,
        host_arch: host,
        target_arch: target,
        runner,
        source_date_epoch,
        jobs: args.jobs,
        timeout,
        debuginfo: false,
    };
    let mut outcomes = Vec::new();
    for test in &selected {
        let started = std::time::Instant::now();
        let result = run_planned_tests(
            &sandbox,
            std::slice::from_ref(test),
            Some(&prepared.path),
            settings,
        );
        let error = result.as_ref().err().map(|error| error.message.clone());
        eprintln!(
            "rpmx: test {} ({}): {} ({:.1}s)",
            test.name,
            test.stage,
            if error.is_some() { "FAILED" } else { "ok" },
            started.elapsed().as_secs_f64()
        );
        outcomes.push(report::TestOutcome {
            name: test.name.clone(),
            stage: test.stage.clone(),
            error,
            duration: started.elapsed(),
        });
    }
    if let Some(path) = args.junit.as_deref() {
        report::write_junit(path, &outcomes).map_err(|error| {
            Error::new(
                codes::TEST_003,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }
    let failed = outcomes
        .iter()
        .filter(|outcome| outcome.error.is_some())
        .count();
    let _ = rpmx_core::root::remove_tree(&prepared.path);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report::test_document(&outcomes))
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
        );
        if failed > 0 {
            return Err(Error::new(codes::TEST_003, format!("{failed} test(s) failed")).reported());
        }
        return Ok(());
    }
    if failed > 0 {
        return Err(Error::new(
            codes::TEST_003,
            format!("{failed} test(s) failed"),
        ));
    }
    Ok(())
}

fn rpms_in(directory: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut rpms = std::fs::read_dir(directory)
        .map_err(|error| {
            Error::new(
                codes::TEST_004,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            Error::new(
                codes::TEST_004,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?;
    rpms.retain(|entry| {
        let path = entry.path();
        path.extension().is_some_and(|extension| extension == "rpm")
            && !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".src.rpm"))
    });
    rpms.sort_by_key(|entry| entry.file_name());
    if rpms.is_empty() {
        return Err(Error::new(
            codes::TEST_004,
            format!("no binary RPMs in {}", directory.display()),
        ));
    }
    Ok(rpms.into_iter().map(|entry| entry.path()).collect())
}

#[cfg(test)]
mod tests {
    use super::rpms_in;
    use rpmx_core::error::codes;

    #[test]
    fn install_dir_reads_binary_rpms_only() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("a.rpm"), b"rpm").expect("a");
        std::fs::write(directory.path().join("a.src.rpm"), b"src").expect("src");
        std::fs::write(directory.path().join("notes.txt"), b"text").expect("txt");
        let rpms = rpms_in(directory.path()).expect("rpms");
        assert_eq!(rpms.len(), 1);
        assert_eq!(rpms[0].file_name().unwrap(), "a.rpm");
    }

    #[test]
    fn an_empty_install_dir_is_a_recipe_error() {
        let directory = tempfile::tempdir().expect("tempdir");
        let error = rpms_in(directory.path()).expect_err("empty");
        assert_eq!(error.code, codes::TEST_004);
    }
}
