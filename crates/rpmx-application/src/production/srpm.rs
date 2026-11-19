use crate::domain::{
    error::codes,
    source_package::{prepare_source_packages, SourcePackageOptions},
    Error, Plan, RecipeBundle,
};
use crate::inputs::PreparedInputs;
use crate::{environment, support};
use crate::{ApplicationConfig, RunReport, SrpmInput};
use std::path::{Path, PathBuf};

pub(crate) fn execute(
    config: &ApplicationConfig,
    input: &SrpmInput,
    output: &Path,
    report: &mut RunReport,
) -> Result<(), Error> {
    if input.arch.as_deref() == Some("all") {
        return super::shared_srpm::execute(config, input, output, report);
    }
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
        let provenance = if bundle.recipe.build.root.is_none()
            && config.root.profile().is_none()
            && config.root.lock().is_none()
        {
            PreparedInputs::default()
        } else {
            PreparedInputs::resolve(
                &bundle,
                &plan,
                target,
                config,
                crate::inputs::TestSelection::new(true, input.include_optional_tests),
            )
            .map_err(|failure| failure.report(report, output))?
        };
        if provenance.root.is_some() {
            report
                .evidence
                .push(provenance.export(output, bundle.digest(), &plan.arch)?);
        }
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
        let provenance = PreparedInputs::resolve(
            &bundle,
            &plan,
            target,
            config,
            crate::inputs::TestSelection::new(true, input.include_optional_tests),
        )
        .map_err(|failure| failure.report(report, output))?;
        let environment = provenance.materialize(target, config)?;
        if provenance.root.is_some() {
            report
                .evidence
                .push(provenance.export(output, bundle.digest(), &plan.arch)?);
        }
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
            &provenance,
            &sources,
            output,
            epoch,
            report,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn package(
    config: &ApplicationConfig,
    bundle: &RecipeBundle,
    plan: &Plan,
    provenance: &PreparedInputs,
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
    let portable = provenance.portable();
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
        root_lock: portable.root.as_ref(),
        sysroot_lock: portable.sysroot.as_ref(),
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
    for mut package in packages {
        if provenance.root.is_some() {
            provenance.embed(&mut package, bundle.digest(), &plan.arch)?;
        }
        let path = crate::emit::source(&package, output, key.as_ref(), timestamp)?;
        report.artifacts.srpms.push(path.clone());
        written.push(path);
    }
    Ok(written)
}
