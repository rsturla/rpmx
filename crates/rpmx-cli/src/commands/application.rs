//! Parse-facing adapters for the shared application lifecycle.
use rpmx_application::{ApplicationConfig, BuildRoot, ProductionPolicy, SigningPolicy};
use rpmx_core::{error::codes, Error};
use std::path::{Path, PathBuf};

pub fn root(
    profile: Option<PathBuf>,
    lock: Option<PathBuf>,
    root: Option<PathBuf>,
    unsafe_host: bool,
) -> Result<BuildRoot, Error> {
    if unsafe_host {
        if profile.is_some() || lock.is_some() || root.is_some() {
            return Err(Error::new(
                codes::ROOT_016,
                "--unsafe-host-root cannot be combined with a profile or saved root",
            ));
        }
        return Ok(BuildRoot::UnsafeHost);
    }
    match (lock, root) {
        (Some(lock), root) => Ok(BuildRoot::Locked { lock, root }),
        (None, Some(_)) => Err(Error::new(codes::ROOT_016, "--root requires --root-lock")),
        (None, None) => Ok(BuildRoot::Automatic { profile }),
    }
}

pub fn policy(args: &crate::cli::PolicyArgs, epoch: Option<u32>) -> ProductionPolicy {
    ProductionPolicy {
        dist_tag: args.dist_tag.clone(),
        build_release: args.build_release,
        source_date_epoch: epoch,
    }
}
pub fn signing(args: &super::OptionalSigningArgs) -> SigningPolicy {
    SigningPolicy {
        signing_key: args.signing_key.clone(),
        signing_passphrase_env: args.signing_passphrase_env.clone(),
        signing_timestamp: args.signing_timestamp,
    }
}
pub fn config(runtime: &super::RuntimeArgs, pipelines: Option<&Path>) -> ApplicationConfig {
    ApplicationConfig {
        cache: runtime.cache.cache.clone(),
        jobs: runtime.jobs,
        timeout: runtime.step_timeout,
        pipelines: pipelines.map(Path::to_path_buf),
        ..ApplicationConfig::default()
    }
}
