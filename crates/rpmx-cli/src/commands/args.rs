use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

#[derive(Debug, clap::Args)]
pub struct BuildArgs {
    /// Recipe to build.
    pub file: PathBuf,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Package an already-staged tree instead of running the full pipeline.
    #[arg(
        long,
        conflicts_with_all = [
            "root_lock", "root", "unsafe_host_root", "run_tests", "test_root_lock",
            "cache", "proxy", "offline", "jobs", "step_timeout", "sbom",
        ]
    )]
    pub stage: Option<PathBuf>,
    /// Output directory for binary RPMs and build-level SBOMs.
    #[arg(long, default_value = ".")]
    pub output: PathBuf,
    /// Architecture for outputs that do not declare one.
    #[arg(long, default_value = rpmx_core::arch::native_arch())]
    pub arch: String,
    /// Runner: auto, bwrap or qemu.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bwrap", "qemu"])]
    pub runner: String,
    /// Reproducible timestamp in Unix seconds (default: package.source-date-epoch).
    #[arg(long)]
    pub source_date_epoch: Option<u32>,
    /// Build one matrix cell.
    #[arg(long)]
    pub cell: Option<String>,
    /// Content-addressed cache (default: $XDG_CACHE_HOME/rpmx/cas).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Content-addressed acquisition proxy.
    #[arg(long)]
    pub proxy: Option<String>,
    /// Do not use the network; fail on a cache miss.
    #[arg(long)]
    pub offline: bool,
    /// Root lock for source assembly and rebuild.
    #[arg(long)]
    pub root_lock: Option<PathBuf>,
    /// Assembled root for source assembly and rebuild.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Use the host root for source assembly (unsafe).
    #[arg(long)]
    pub unsafe_host_root: bool,
    /// Parallelism exposed as JOBS.
    #[arg(long, default_value_t = NonZeroU32::new(1).unwrap())]
    pub jobs: NonZeroU32,
    /// Wall-clock limit for each step in seconds (must be at least 1).
    #[arg(long, default_value_t = NonZeroU64::new(3600).unwrap())]
    pub step_timeout: NonZeroU64,
    /// Run build-stage and installed-stage tests.
    #[arg(long)]
    pub run_tests: bool,
    /// Override the build-level SPDX SBOM path.
    #[arg(long)]
    pub sbom: Option<PathBuf>,
    /// Use a pinned test-root lock instead of resolving one.
    #[arg(long, requires = "run_tests")]
    pub test_root_lock: Option<PathBuf>,
    /// Sign the SRPM and binary RPMs with this ASCII-armored OpenPGP secret key.
    #[arg(long)]
    pub signing_key: Option<PathBuf>,
    /// Read the signing key passphrase from this environment variable.
    #[arg(long, requires = "signing_key")]
    pub signing_passphrase_env: Option<String>,
    /// Signature creation time in Unix seconds (default: source date epoch, then now).
    #[arg(long, requires = "signing_key")]
    pub signing_timestamp: Option<u32>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct RebuildArgs {
    /// Source RPM to rebuild.
    pub file: PathBuf,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Output directory.
    #[arg(long, default_value = ".")]
    pub output: PathBuf,
    /// Architecture for outputs that do not declare one.
    #[arg(long, default_value = rpmx_core::arch::native_arch())]
    pub arch: String,
    /// Runner: auto, bwrap or qemu.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bwrap", "qemu"])]
    pub runner: String,
    /// Build one matrix cell (required when the plan has several).
    #[arg(long)]
    pub cell: Option<String>,
    /// Parallelism exposed as JOBS.
    #[arg(long, default_value_t = NonZeroU32::new(1).unwrap())]
    pub jobs: NonZeroU32,
    /// Wall-clock limit for each step in seconds (must be at least 1).
    #[arg(long, default_value_t = NonZeroU64::new(3600).unwrap())]
    pub step_timeout: NonZeroU64,
    /// Run steps inside this builder image instead of host bwrap.
    #[arg(long, conflicts_with = "root_lock")]
    pub root_image: Option<String>,
    /// Root lock describing the build root.
    #[arg(long)]
    pub root_lock: Option<PathBuf>,
    /// Assembled root directory.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Run build-stage tests after the build.
    #[arg(long)]
    pub run_tests: bool,
    /// Override the build-level SPDX SBOM path.
    #[arg(long)]
    pub sbom: Option<PathBuf>,
    /// Use a pinned test-root lock instead of resolving one.
    #[arg(long, requires = "run_tests")]
    pub test_root_lock: Option<PathBuf>,
    /// Sign the built RPMs with this ASCII-armored OpenPGP secret key.
    #[arg(long)]
    pub signing_key: Option<PathBuf>,
    /// Read the signing key passphrase from this environment variable.
    #[arg(long, requires = "signing_key")]
    pub signing_passphrase_env: Option<String>,
    /// Signature creation time in Unix seconds (default: source date epoch, then now).
    #[arg(long, requires = "signing_key")]
    pub signing_timestamp: Option<u32>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
    /// Suppress reporting when this command is used as a pipeline step.
    #[arg(skip)]
    pub quiet: bool,
}

#[derive(Debug, clap::Args)]
pub struct LintArgs {
    /// Recipe to check.
    pub file: PathBuf,
    /// Target architecture used for templates.
    #[arg(long, default_value = rpmx_core::arch::native_arch())]
    pub arch: String,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct PlanArgs {
    /// Recipe to plan.
    pub file: PathBuf,
    /// Target architecture used for templates.
    #[arg(long, default_value = rpmx_core::arch::native_arch())]
    pub arch: String,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Emit the plan as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct FetchArgs {
    /// Recipe whose sources should be fetched.
    pub file: PathBuf,
    /// CAS directory (default: $XDG_CACHE_HOME/rpmx/cas).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Content-addressed acquisition proxy.
    #[arg(long)]
    pub proxy: Option<String>,
    /// Do not use the network; fail on a cache miss.
    #[arg(long)]
    pub offline: bool,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct SourceArgs {
    /// Recipe to assemble.
    pub file: PathBuf,
    /// Target architecture used for templates.
    #[arg(long, default_value = rpmx_core::arch::native_arch())]
    pub arch: String,
    /// Runner: auto, bwrap or qemu.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bwrap", "qemu"])]
    pub runner: String,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// CAS directory (default: $XDG_CACHE_HOME/rpmx/cas).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Content-addressed acquisition proxy.
    #[arg(long)]
    pub proxy: Option<String>,
    /// Do not use the network; fail on a cache miss.
    #[arg(long)]
    pub offline: bool,
    /// Root lock for source assembly.
    #[arg(long)]
    pub root_lock: Option<PathBuf>,
    /// Assembled root for source assembly.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Use the host root for source assembly (unsafe).
    #[arg(long)]
    pub unsafe_host_root: bool,
    /// Output directory for RPMX-SOURCE.tar.gz.
    #[arg(long, default_value = ".")]
    pub output: PathBuf,
    /// Reproducible timestamp in Unix seconds (default: package.source-date-epoch).
    #[arg(long)]
    pub source_date_epoch: Option<u32>,
    /// Parallelism exposed as JOBS.
    #[arg(long, default_value_t = NonZeroU32::new(1).unwrap())]
    pub jobs: NonZeroU32,
    /// Wall-clock limit for each step in seconds (must be at least 1).
    #[arg(long, default_value_t = NonZeroU64::new(3600).unwrap())]
    pub step_timeout: NonZeroU64,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
    /// Suppress reporting when this command is used as a pipeline step.
    #[arg(skip)]
    pub quiet: bool,
}

#[derive(Debug, clap::Args)]
pub struct SrpmArgs {
    /// Recipe to package.
    pub file: PathBuf,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Directory containing the source snapshot (flat files).
    #[arg(long)]
    pub sources: PathBuf,
    /// Output directory.
    #[arg(long, default_value = ".")]
    pub output: PathBuf,
    /// Build architecture recorded in the source header.
    #[arg(long, default_value = rpmx_core::arch::native_arch())]
    pub arch: String,
    /// Reproducible timestamp in Unix seconds (default: package.source-date-epoch).
    #[arg(long)]
    pub source_date_epoch: Option<u32>,
    /// Write one matrix cell (default: every cell).
    #[arg(long)]
    pub cell: Option<String>,
    /// Root lock to embed in the SRPM.
    #[arg(long)]
    pub root_lock: Option<PathBuf>,
    /// Sign the SRPM with this ASCII-armored OpenPGP secret key.
    #[arg(long)]
    pub signing_key: Option<PathBuf>,
    /// Read the signing key passphrase from this environment variable.
    #[arg(long, requires = "signing_key")]
    pub signing_passphrase_env: Option<String>,
    /// Signature creation time in Unix seconds (default: now).
    #[arg(long, requires = "signing_key")]
    pub signing_timestamp: Option<u32>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
    /// Suppress reporting when this command is used as a pipeline step.
    #[arg(skip)]
    pub quiet: bool,
}

pub struct RootResolveArgs<'a> {
    pub profile: &'a Path,
    pub output: Option<&'a Path>,
    pub work: Option<&'a Path>,
    pub keep: bool,
    pub cache: Option<&'a Path>,
    pub json: bool,
}

pub struct RootAssembleArgs<'a> {
    pub lock: &'a Path,
    pub root: Option<&'a Path>,
    pub output: Option<&'a Path>,
    pub cache: Option<&'a Path>,
    pub work: Option<&'a Path>,
    pub keep: bool,
    pub json: bool,
}

pub struct RootDigestArgs<'a> {
    pub root: &'a Path,
    pub json: bool,
}

pub struct RootExportArgs<'a> {
    pub lock: &'a Path,
    pub output: Option<&'a Path>,
}

pub struct RootImportArgs<'a> {
    pub manifest: &'a Path,
    pub profile: &'a str,
    pub arch: Option<&'a str>,
    pub releasever: &'a str,
    pub output: Option<&'a Path>,
}
