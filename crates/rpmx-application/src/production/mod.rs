use crate::domain::{error::codes, Error, Plan, RecipeBundle};
use crate::support;
use crate::{ApplicationConfig, BuildInput, RunReport};
use std::path::Path;

pub(crate) mod build;
pub(crate) mod rebuild;
pub(crate) mod source;
pub(crate) mod srpm;
mod tests;

pub(crate) fn index(
    config: &ApplicationConfig,
    output: &Path,
    epoch: u32,
    report: &mut RunReport,
) -> Result<(), Error> {
    let key = support::load_signing_key(
        config.signing.signing_key.as_deref(),
        config.signing.signing_passphrase_env.as_deref(),
    )?;
    let timestamp =
        support::signature_timestamp(config.signing.signing_timestamp, epoch, key.as_ref())?;
    let generated = rpmx_rpm::repo::generate(rpmx_rpm::repo::RepoOptions {
        rpms: output,
        output,
        signing_key: key.as_ref(),
        timestamp: timestamp.unwrap_or(epoch),
    })?;
    report.artifacts.repository.push(generated.repomd);
    if let Some(path) = generated.signature {
        report.artifacts.repository.push(path);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn package_stage(
    config: &ApplicationConfig,
    input: &BuildInput,
    bundle: &RecipeBundle,
    plan: &Plan,
    target: crate::domain::Arch,
    stage: &Path,
    output: &Path,
    report: &mut RunReport,
) -> Result<(), Error> {
    if plan.cells.len() != 1 {
        return Err(Error::new(
            codes::BUILD_003,
            "staged packaging requires exactly one cell",
        ));
    }
    let cell = &plan.cells[0];
    if input.build_tests && !cell.build_tests.is_empty() {
        return Err(Error::new(codes::TEST_005, "staged packaging has no build workspace for build.tests; use --no-tests to package the tree, then run standalone RPM acceptance"));
    }
    let epoch = support::source_date_epoch(&bundle.recipe, config.policy.source_date_epoch);
    let inventory = crate::domain::components::discover(stage, &[cell])?;
    let packages = crate::domain::binary_package::prepare_binary_packages(
        crate::domain::binary_package::BinaryPackageOptions {
            recipe: &bundle.recipe,
            cell,
            stage,
            arch: target.as_str(),
            source_date_epoch: epoch,
            bundled: &inventory,
        },
    )?;
    if input.lint {
        crate::package_lint::enforce(&crate::package_lint::check(&packages))?;
    }
    let key = support::load_signing_key(
        config.signing.signing_key.as_deref(),
        config.signing.signing_passphrase_env.as_deref(),
    )?;
    let timestamp =
        support::signature_timestamp(config.signing.signing_timestamp, epoch, key.as_ref())?;
    std::fs::create_dir_all(output).map_err(|e| {
        Error::new(
            codes::BUILD_001,
            format!("cannot create {}: {e}", output.display()),
        )
    })?;
    for package in packages {
        report.artifacts.rpms.push(crate::emit::binary(
            &package,
            output,
            key.as_ref(),
            timestamp,
        )?);
    }
    if input.index {
        index(config, output, epoch, report)?;
    }
    Ok(())
}
