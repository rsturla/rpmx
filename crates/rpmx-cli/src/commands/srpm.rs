use rpmx_core::error::codes;
use rpmx_core::root::load_lock;
use rpmx_core::source_package::{prepare_source_packages, SourcePackageOptions};
use rpmx_core::Error;
use rpmx_rpm::{write_srpm, SrpmFile, SrpmSpec};

use crate::commands::{load_plan, reconcile_policy, step_root, PlanPolicy, SrpmArgs};

pub fn run(args: SrpmArgs<'_>) -> Result<Vec<std::path::PathBuf>, Error> {
    let SrpmArgs {
        file,
        sources,
        output,
        arch,
        source_date_epoch,
        cell,
        pipeline_dir,
        policy,
        root_lock,
        signing_key,
        signing_passphrase_env,
        signing_timestamp,
        quiet,
        json,
    } = args;
    let signing = crate::commands::load_signing_key(signing_key, signing_passphrase_env)?;
    let root_lock = match root_lock {
        Some(path) => {
            let lock = load_lock(path)?;
            if lock.root_digest.is_none() {
                return Err(Error::new(
                    codes::ROOT_016,
                    format!(
                        "{} has no root_digest; run `rpmx root assemble` before embedding it",
                        path.display()
                    ),
                ));
            }
            Some(lock)
        }
        None => None,
    };
    // A root lock is a target-distro statement. It supplies the dist tag when
    // the profile and command line did not, and conflicts are errors rather
    // than silently producing contradictory provenance.
    let policy = match root_lock.as_ref().and_then(|lock| lock.dist_tag.as_deref()) {
        Some(tag) => reconcile_policy(
            policy,
            PlanPolicy {
                dist_tag: Some(tag),
                build_release: None,
            },
        )?,
        None => policy,
    };
    let (recipe, mut plan) = load_plan(file, arch, pipeline_dir, false, policy)?;
    if let Some(id) = cell {
        plan.cells.retain(|planned| planned.id == id);
        if plan.cells.is_empty() {
            return Err(Error::new(codes::BUILD_002, format!("no such cell '{id}'")));
        }
    }
    let source_date_epoch = crate::commands::source_date_epoch(&recipe, source_date_epoch);
    let signature_timestamp = crate::commands::signature_timestamp(
        signing_timestamp,
        source_date_epoch,
        signing.as_ref(),
    )?;
    let recipe_bytes = std::fs::read(file).map_err(|error| {
        Error::new(
            codes::SRPM_001,
            format!("cannot read {}: {error}", file.display()),
        )
    })?;
    let root = step_root(pipeline_dir, file);
    let debuginfo = root_lock.as_ref().is_some_and(|lock| lock.debuginfo);
    let sbom = root_lock.as_ref().map(|lock| lock.sbom).unwrap_or(true);
    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &recipe,
        plan: &plan,
        recipe_bytes: &recipe_bytes,
        step_root: root.as_deref(),
        sources,
        source_date_epoch,
        root_lock: root_lock.as_ref(),
        debuginfo,
        sbom,
    })?;

    std::fs::create_dir_all(output).map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot create {}: {error}", output.display()),
        )
    })?;

    let mut written = Vec::new();
    for package in packages {
        let path = output.join(format!(
            "{}-{}-{}.src.rpm",
            package.name, package.version, package.release
        ));
        let spec = SrpmSpec {
            name: package.name,
            version: package.version,
            release: package.release,
            epoch: package.epoch,
            arch: arch.to_string(),
            summary: package.summary,
            license: package.license,
            description: package.description,
            source_date_epoch,
            files: package
                .files
                .into_iter()
                .map(|file| SrpmFile {
                    path: file.path,
                    data: file.data,
                    mode: file.mode,
                })
                .collect(),
        };
        write_srpm(&spec, &path)?;
        if let Some(key) = &signing {
            rpmx_rpm::sign::sign_package(&path, key, signature_timestamp)?;
        }
        written.push(path);
    }

    if !quiet {
        if json {
            let value = serde_json::json!({
                "ok": true,
                "srpms": written.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
                "signing_key": signing.as_ref().map(|key| key.fingerprint()),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&value)
                    .map_err(|error| Error::new(codes::CLI_003, error.to_string()))?
            );
        } else {
            for path in &written {
                println!("{}", path.display());
            }
        }
    }
    Ok(written)
}
