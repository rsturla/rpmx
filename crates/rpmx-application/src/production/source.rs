use crate::domain::{
    error::codes,
    source::{assemble, SourceOptions},
    Error, Plan, RecipeBundle,
};
use crate::{
    environment::{self, Environment},
    support::{self, WorkDir},
};
use crate::{ApplicationConfig, RunReport, SourceInput};
use std::path::Path;

pub(crate) fn execute(
    config: &ApplicationConfig,
    input: &SourceInput,
    output: &Path,
    report: &mut RunReport,
) -> Result<(), Error> {
    let target = environment::target(&input.recipe, input.arch.as_deref(), config)?;
    let policy = environment::policy(config)?;
    let (bundle, plan) = environment::plan(&input.recipe, target, config, policy.plan_policy())?;
    let environment = Environment::prepare(&bundle, &plan, target, config)?;
    if input.locked {
        support::lock::verify(
            &bundle,
            &plan,
            policy.plan_policy(),
            config.root.profile(),
            target.as_str(),
            config.policy.source_date_epoch,
            environment.lock(),
            environment.sysroot.as_ref().map(|root| root.lock()),
            true,
            config.cache.as_deref(),
        )?;
    }
    assemble_sources(
        config,
        &bundle,
        &plan,
        &environment,
        &output.join("source"),
        output,
        report,
    )?;
    Ok(())
}

pub(crate) fn assemble_sources(
    config: &ApplicationConfig,
    bundle: &RecipeBundle,
    plan: &Plan,
    env: &Environment,
    output: &Path,
    evidence_root: &Path,
    report: &mut RunReport,
) -> Result<u32, Error> {
    let cache = config
        .cache
        .clone()
        .map(Ok)
        .unwrap_or_else(support::default_cache)?;
    let proxy = config
        .proxy
        .clone()
        .or_else(|| std::env::var("RPMX_PROXY").ok())
        .or_else(|| std::env::var("RPMX_ACQUISITION_PROXY").ok());
    let work = WorkDir::new("source")?;
    let workspace = work.path().join("build");
    let stage = work.path().join("stage");
    std::fs::create_dir_all(output).map_err(|e| {
        Error::new(
            codes::BUILD_006,
            format!("cannot create {}: {e}", output.display()),
        )
    })?;
    let base = bundle.root.parent().unwrap_or_else(|| Path::new("."));
    let evidence = report
        .evidence_directory(evidence_root)
        .join("source")
        .join(env.target.as_str());
    std::fs::create_dir_all(&evidence)
        .map_err(|error| Error::new(codes::SOURCE_030, error.to_string()))?;
    let logs = crate::domain::runner::OutputLogs {
        stdout: std::fs::File::create(evidence.join("stdout.log"))
            .map_err(|error| Error::new(codes::SOURCE_030, error.to_string()))?,
        stderr: std::fs::File::create(evidence.join("stderr.log"))
            .map_err(|error| Error::new(codes::SOURCE_030, error.to_string()))?,
    };
    report.evidence.push(evidence);
    let assembled = assemble(SourceOptions {
        logs: &logs,
        recipe: &bundle.recipe,
        plan,
        cache: &cache,
        proxy: proxy.as_deref(),
        offline: config.offline,
        base_dir: base,
        workspace: &workspace,
        stage: &stage,
        prefix: env.prefix(),
        libdir: env.libdir(),
        output,
        source_date_epoch: config.policy.source_date_epoch,
        source_date_epoch_default: bundle.source_date_epoch_default,
        jobs: config.jobs,
        step_timeout: config.timeout,
        root: env.root(),
        appliance: env.appliance.as_deref(),
        exec: env.exec,
        sysroot: env.sysroot.as_ref().map(|root| root.path()),
        cross: env.cross.as_ref(),
        runner: &config.runner,
        recipe_dir: base,
    })
    .map_err(|failure| {
        report
            .secondary
            .extend(failure.secondary.iter().map(crate::Diagnostic::from));
        *failure.primary
    })?;
    report.artifacts.sources.push(assembled.snapshot);
    if let Some(root) = env.lock() {
        let path = output.join("source-root.lock.json");
        crate::domain::root::write_lock(&path, &root.redacted())?;
        report.evidence.push(path);
    }
    Ok(assembled.source_date_epoch)
}
