use crate::domain::{
    error::codes,
    source_package::{prepare_source_packages, SourcePackageOptions},
    Error, Plan, RecipeBundle,
};
use crate::{
    environment::{self, Environment},
    support,
};
use crate::{ApplicationConfig, RunReport, SrpmInput};
use std::path::{Path, PathBuf};

pub(crate) fn execute(
    config: &ApplicationConfig,
    input: &SrpmInput,
    output: &Path,
    report: &mut RunReport,
) -> Result<(), Error> {
    let target = environment::target(&input.recipe, input.arch.as_deref(), config)?;
    let policy = environment::policy(config)?;
    let (bundle, mut plan) =
        environment::plan(&input.recipe, target, config, policy.plan_policy())?;
    if let Some(cell) = &input.cell {
        plan.cells.retain(|planned| planned.id == *cell);
    }
    if plan.cells.is_empty() {
        return Err(Error::new(codes::BUILD_002, "no selected cells"));
    }
    if let Some(sources) = &input.sources {
        // Packaging an existing source snapshot does not execute commands and
        // does not invent a build environment for a recipe that declares none.
        let environment = if bundle.recipe.build.root.is_none()
            && config.root.profile().is_none()
            && config.root.lock().is_none()
        {
            None
        } else {
            Some(Environment::prepare(&bundle, &plan, target, config)?)
        };
        let provenance = SourceBuildProvenance::from_environment(environment.as_ref());
        let epoch = config
            .policy
            .source_date_epoch
            .or(bundle.recipe.package.source_date_epoch)
            .or(bundle.source_date_epoch_default)
            .unwrap_or(0);
        package(
            config,
            &bundle,
            &plan,
            &provenance,
            sources,
            output,
            epoch,
            report,
        )?;
    } else {
        let environment = Environment::prepare(&bundle, &plan, target, config)?;
        let sources = output.join("source");
        let epoch = super::source::assemble_sources(
            config,
            &bundle,
            &plan,
            &environment,
            &sources,
            output,
            report,
        )?;
        package(
            config,
            &bundle,
            &plan,
            &SourceBuildProvenance::from_environment(Some(&environment)),
            &sources,
            output,
            epoch,
            report,
        )?;
    }
    Ok(())
}

/// Persisted source-package provenance, not an execution root handle.
pub(crate) struct SourceBuildProvenance {
    root: Option<crate::domain::root::RootLock>,
    sysroot: Option<crate::domain::root::RootLock>,
    cross: Option<crate::domain::guest::CrossToolchain>,
}
impl SourceBuildProvenance {
    pub fn from_environment(environment: Option<&Environment>) -> Self {
        Self {
            root: environment.and_then(|env| env.lock().cloned()),
            sysroot: environment
                .and_then(|env| env.sysroot.as_ref().map(|root| root.lock().clone())),
            cross: environment.and_then(|env| env.cross.clone()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn package(
    config: &ApplicationConfig,
    bundle: &RecipeBundle,
    plan: &Plan,
    provenance: &SourceBuildProvenance,
    sources: &Path,
    output: &Path,
    epoch: u32,
    report: &mut RunReport,
) -> Result<Vec<PathBuf>, Error> {
    let key = support::load_signing_key(
        config.signing.signing_key.as_deref(),
        config.signing.signing_passphrase_env.as_deref(),
    )?;
    let timestamp =
        support::signature_timestamp(config.signing.signing_timestamp, epoch, key.as_ref())?;
    let recipe_bytes = bundle.root_file().bytes.clone();
    let includes: Vec<_> = bundle
        .included_files()
        .map(|file| crate::domain::source_package::RecipeInclude {
            name: file.name.clone(),
            bytes: file.bytes.clone(),
            mode: file.mode,
        })
        .collect();
    let steps = support::step_root(config.pipelines.as_deref(), &bundle.root);
    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &bundle.recipe,
        plan,
        recipe_bytes: &recipe_bytes,
        recipe_mode: bundle.root_file().mode,
        recipe_digest: Some(bundle.digest()),
        recipe_includes: &includes,
        step_root: steps.as_deref(),
        sources,
        source_date_epoch: epoch,
        root_lock: provenance.root.as_ref(),
        sysroot_lock: provenance.sysroot.as_ref(),
        cross: provenance.cross.as_ref(),
        sbom: provenance
            .root
            .as_ref()
            .map(|lock| lock.sbom)
            .unwrap_or(true),
        replaced: None,
    })?;
    std::fs::create_dir_all(output).map_err(|e| {
        Error::new(
            codes::SRPM_002,
            format!("cannot create {}: {e}", output.display()),
        )
    })?;
    let mut written = Vec::new();
    for package in packages {
        let path = crate::emit::source(&package, output, key.as_ref(), timestamp)?;
        report.artifacts.srpms.push(path.clone());
        written.push(path);
    }
    Ok(written)
}
