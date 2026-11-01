mod args;
pub mod autoroot;
pub mod build;
pub mod cache;
pub mod dispatch;
pub mod emit;
pub mod fetch;
pub mod lint;
mod loader;
pub mod pipeline;
pub mod plan;
mod policy;
pub mod rebuild;
pub mod repo;
pub mod report;
pub mod root;
pub mod sign;
mod signing;
pub mod source;
pub mod srpm;

pub use args::*;
pub use loader::{load_plan, step_root};

use std::num::NonZeroU32;
use std::path::PathBuf;

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
    pub fn new() -> Result<Self, rpmx_core::Error> {
        let temporary = tempfile::tempdir().map_err(|error| {
            rpmx_core::Error::new(
                rpmx_core::error::codes::BUILD_005,
                format!("cannot create work directory: {error}"),
            )
        })?;
        if rpmx_core::output::keep_workdir() {
            let path = temporary.keep();
            eprintln!("work directory: {}", path.display());
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
pub use policy::{reconcile_policy, resolve_policy, PlanPolicy, PolicyInput};
pub use signing::{load_signing_key, signature_timestamp, signing_passphrase, source_date_epoch};
