use std::collections::BTreeMap;
use std::path::Path;

use crate::domain::{error::codes, source_package, Arch, Error};
use crate::{environment, support, ApplicationConfig, RunReport, SrpmInput};

/// Resolve target inputs separately, then package one shared source snapshot.
pub(super) fn execute(
    config: &ApplicationConfig,
    input: &SrpmInput,
    output: &Path,
    report: &mut RunReport,
) -> Result<(), Error> {
    if config.root.lock().is_some()
        || config.root.path().is_some()
        || config.root.unsafe_host()
        || config.offline
    {
        return Err(Error::new(
            codes::SRPM_006,
            "--arch all requires automatic root resolution and network access",
        ));
    }
    let bundle = crate::domain::load_recipe_bundle(&input.recipe)?;
    let mut arches =
        rpmx_root::recipe::declared_arches(&bundle.recipe, &bundle.root, config.root.profile())?
            .ok_or_else(|| {
                Error::new(
                    codes::SRPM_006,
                    "--arch all requires a profile with target architectures",
                )
            })?;
    arches.sort();
    arches.dedup();
    let native = Arch::current()?;
    if !arches.iter().any(|arch| arch == native.as_str()) {
        return Err(Error::new(
            codes::SRPM_006,
            "source preparation requires the native architecture in the profile",
        ));
    }
    let policy = environment::policy(config)?;
    let mut prepared = Vec::new();
    for arch in &arches {
        let arch = Arch::parse(arch)?;
        let (_, mut plan) = environment::plan(&input.recipe, arch, config, policy.plan_policy())?;
        if let Some(cell) = &input.cell {
            plan.cells.retain(|planned| &planned.id == cell);
        }
        if plan.cells.is_empty() {
            return Err(Error::new(codes::BUILD_002, "no selected cells"));
        }
        let inputs = crate::inputs::PreparedInputs::resolve(
            &bundle,
            &plan,
            arch,
            config,
            crate::inputs::TestSelection::new(true, input.include_optional_tests),
        )
        .map_err(|failure| failure.report(report, output))?;
        if inputs.cross.is_some() {
            return Err(Error::new(
                codes::SRPM_006,
                "--arch all prepares native target builds; use a single target for cross SRPMs",
            ));
        }
        prepared.push((plan, inputs));
    }
    std::fs::create_dir_all(output).map_err(file_error)?;
    let locks = output.join("locks");
    std::fs::create_dir_all(&locks).map_err(file_error)?;
    for (plan, provenance) in &prepared {
        report
            .evidence
            .push(provenance.export(output, bundle.digest(), &plan.arch)?);
        let path = locks.join(format!("root-{}.lock.json", plan.arch));
        rpmx_root::root::write_lock(
            &path,
            &provenance.root.as_ref().expect("resolved root").redacted(),
        )?;
        report.evidence.push(path);
        if let Some(root) = &provenance.sysroot {
            let path = locks.join(format!("sysroot-{}.lock.json", plan.arch));
            rpmx_root::root::write_lock(&path, &root.redacted())?;
            report.evidence.push(path);
        }
    }
    let sources = input
        .sources
        .clone()
        .unwrap_or_else(|| output.join("source"));
    let epoch = if input.sources.is_some() {
        config
            .policy
            .source_date_epoch
            .or(bundle.recipe.package.source_date_epoch)
            .or(bundle.source_date_epoch_default)
            .unwrap_or(0)
    } else {
        let (plan, provenance) = prepared
            .iter()
            .find(|(plan, _)| plan.arch == native.as_str())
            .expect("native target checked");
        let commands = !bundle.recipe.patches.is_empty()
            || plan
                .source_assembly
                .as_ref()
                .is_some_and(|source| !source.steps.is_empty());
        let env = if commands {
            provenance.materialize(native, config)?
        } else {
            // Extraction and context copies do not execute recipe commands.
            environment::Environment {
                target: native,
                exec: native,
                selected: None,
                lock_path: None,
                sysroot: None,
                cross: None,
                appliance: None,
            }
        };
        super::source::assemble_sources(config, &bundle, plan, &env, &sources, output, report)?
    };
    let includes = bundle
        .included_files()
        .map(|file| source_package::RecipeInclude {
            name: file.name.clone(),
            bytes: file.bytes.clone(),
            mode: file.mode,
        })
        .collect::<Vec<_>>();
    let source_plan = serde_json::to_value(&prepared[0].0.source_assembly)
        .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?;
    for (plan, _) in &prepared {
        if serde_json::to_value(&plan.source_assembly)
            .map_err(|error| Error::new(codes::SRPM_006, error.to_string()))?
            != source_plan
        {
            return Err(Error::new(
                codes::SRPM_006,
                "source assembly differs across targets; prepare architecture-specific SRPMs",
            ));
        }
    }
    let steps = support::step_root(config.pipelines.as_deref(), &bundle.root);
    let mut packages = BTreeMap::<_, Vec<_>>::new();
    for (plan, provenance) in &prepared {
        for package in
            source_package::prepare_source_packages(source_package::SourcePackageOptions {
                recipe: &bundle.recipe,
                plan,
                recipe_bytes: &bundle.root_file().bytes,
                recipe_mode: bundle.root_file().mode,
                recipe_digest: Some(bundle.digest()),
                recipe_includes: &includes,
                step_root: steps.as_deref(),
                sources: &sources,
                source_date_epoch: epoch,
                // Resolved dependencies describe a build, not its sources.
                // The caller transports locks/ independently of this SRPM.
                root_lock: None,
                sysroot_lock: None,
                cross: None,
                sbom: provenance.root.as_ref().is_some_and(|root| root.sbom),
                replaced: None,
            })?
        {
            packages
                .entry((
                    package.name.clone(),
                    package.version.clone(),
                    package.release.clone(),
                ))
                .or_default()
                .push(package);
        }
    }
    let key = support::load_signing_key(
        config.signing.signing_key.as_deref(),
        config.signing.signing_passphrase_env.as_deref(),
    )?;
    let timestamp =
        support::signature_timestamp(config.signing.signing_timestamp, epoch, key.as_ref())?;
    for variants in packages.into_values() {
        if variants.len() != arches.len() {
            return Err(Error::new(
                codes::SRPM_006,
                "matrix cells differ across targets",
            ));
        }
        let package = source_package::merge_target_packages(variants)?;
        report.artifacts.srpms.push(crate::emit::source(
            &package,
            output,
            key.as_ref(),
            timestamp,
        )?);
    }
    Ok(())
}

fn file_error(error: std::io::Error) -> Error {
    Error::new(codes::SRPM_006, error.to_string())
}
