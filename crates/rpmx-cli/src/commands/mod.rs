pub mod appliance;
mod application;
mod args;
pub mod bootstrap;
pub mod build;
pub mod bump;
pub mod cache;
pub mod dispatch;
pub mod doctor;
pub mod explain;
pub mod fetch;
pub mod init;
pub mod lint;
pub mod lock;
pub mod plan;
mod policy;
pub mod rebuild;
pub mod repo;
pub mod report;
pub mod root;
pub mod shell;
pub mod sign;
mod signing;
pub mod source;
pub mod srpm;
pub mod steps;
pub mod testcmd;

pub use args::*;
pub use rpmx_base::{io::WorkDir, output::default_jobs};
pub use rpmx_root::recipe::load_plan;

use std::path::PathBuf;

/// The architecture a read-only command should plan for.
///
/// An explicit `--arch` wins; otherwise a single-target profile supplies it,
/// and the host is the last resort. Every command uses this so a cross recipe
/// is inspected as the architecture it builds.
pub fn command_arch(
    file: &std::path::Path,
    explicit: Option<&str>,
    profile: Option<&std::path::Path>,
) -> Result<String, rpmx_core::Error> {
    if let Some(arch) = explicit {
        return Ok(arch.to_string());
    }
    let declared = rpmx_core::load_recipe_bundle(file)?;
    Ok(
        rpmx_root::recipe::declared_arch(&declared.recipe, &declared.root, profile)?
            .unwrap_or_else(|| rpmx_core::arch::native_arch().to_string()),
    )
}

/// Default output directory for a build: `./packages/<arch>`.
pub fn default_output(arch: &str) -> PathBuf {
    PathBuf::from("packages").join(arch)
}

pub use policy::{resolve_policy, PlanPolicy, PolicyInput};
pub use signing::{load_signing_key, signing_passphrase, source_date_epoch};

#[cfg(test)]
mod tests {
    use super::default_output;
    use std::path::PathBuf;

    #[test]
    fn the_output_directory_is_arch_scoped() {
        assert_eq!(default_output("x86_64"), PathBuf::from("packages/x86_64"));
    }
}
