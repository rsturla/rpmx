pub mod appliance;
mod args;
pub mod autoroot;
pub mod build;
pub mod bump;
pub mod cache;
pub mod dispatch;
pub mod doctor;
pub mod emit;
pub mod explain;
pub mod fetch;
pub mod init;
pub mod lint;
mod loader;
pub mod lock;
pub mod package_lint;
pub mod pipeline;
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
pub use loader::{load_plan, step_root};

use std::num::NonZeroU32;
use std::path::PathBuf;

/// The architecture recorded in an RPM or SRPM header.
///
/// `rpmx rebuild` uses it so a cross SRPM rebuilds for its target without an
/// `--arch` flag.
pub fn srpm_arch(path: &std::path::Path) -> Result<String, rpmx_core::Error> {
    rpmx_rpm::package_arch(path)
}

/// Parallelism used when `--jobs` is not given.
pub fn default_jobs() -> NonZeroU32 {
    let detected = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, 64);
    NonZeroU32::new(detected as u32).unwrap_or(NonZeroU32::MIN)
}

/// Default output directory for a build: `./packages/<arch>`.
pub fn default_output(arch: &str) -> PathBuf {
    PathBuf::from("packages").join(arch)
}

/// A temporary work directory that survives the command when
/// `--keep-workdir` is set, and prints where it is.
pub struct WorkDir {
    path: PathBuf,
    _temporary: Option<tempfile::TempDir>,
}

impl WorkDir {
    /// `context` names the work the directory belongs to, so two directories
    /// kept by one command are distinguishable.
    pub fn new(context: &str) -> Result<Self, rpmx_core::Error> {
        let temporary = tempfile::tempdir().map_err(|error| {
            rpmx_core::Error::new(
                rpmx_core::error::codes::BUILD_005,
                format!("cannot create work directory: {error}"),
            )
        })?;
        if rpmx_core::output::keep_workdir() {
            let path = temporary.keep();
            rpmx_core::output::phase("work", format!("{context}: {}", path.display()));
            Ok(Self {
                path,
                _temporary: None,
            })
        } else {
            let path = temporary.path().to_path_buf();
            Ok(Self {
                path,
                _temporary: Some(temporary),
            })
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

pub use policy::{reconcile_policy, resolve_policy, PlanPolicy, PolicyInput};
pub use signing::{load_signing_key, signature_timestamp, signing_passphrase, source_date_epoch};

#[cfg(test)]
mod tests {
    use super::{default_jobs, default_output};
    use std::path::PathBuf;

    #[test]
    fn the_output_directory_is_arch_scoped() {
        assert_eq!(default_output("x86_64"), PathBuf::from("packages/x86_64"));
    }

    #[test]
    fn the_default_job_count_is_non_zero() {
        assert!(default_jobs().get() >= 1);
    }
}
