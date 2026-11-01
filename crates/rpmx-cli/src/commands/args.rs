use std::path::{Path, PathBuf};

use crate::commands::PlanPolicy;

pub struct SrpmArgs<'a> {
    pub file: &'a Path,
    pub sources: &'a Path,
    pub output: &'a Path,
    pub arch: &'a str,
    pub source_date_epoch: Option<u32>,
    pub cell: Option<&'a str>,
    pub pipeline_dir: Option<&'a Path>,
    pub policy: PlanPolicy<'a>,
    pub root_lock: Option<&'a Path>,
    pub signing_key: Option<&'a Path>,
    pub signing_passphrase_env: Option<&'a str>,
    pub signing_timestamp: Option<u32>,
    pub quiet: bool,
    pub json: bool,
}

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
    #[arg(long, default_value_t = 1)]
    pub jobs: u32,
    /// Wall-clock limit for each step in seconds (0 disables).
    #[arg(long, default_value_t = 3600)]
    pub step_timeout: u64,
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

pub struct RebuildArgs<'a> {
    pub file: &'a Path,
    pub output: &'a Path,
    pub arch: &'a str,
    pub runner: &'a str,
    pub cell: Option<&'a str>,
    pub policy: PlanPolicy<'a>,
    pub jobs: u32,
    pub step_timeout: u64,
    pub root_image: Option<&'a str>,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub run_tests: bool,
    pub sbom: Option<&'a Path>,
    pub test_root_lock: Option<&'a Path>,
    pub signing_key: Option<&'a Path>,
    pub signing_passphrase_env: Option<&'a str>,
    pub signing_timestamp: Option<u32>,
    pub quiet: bool,
    pub json: bool,
}

pub struct FetchArgs<'a> {
    pub file: &'a Path,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub json: bool,
}

pub struct SourceArgs<'a> {
    pub file: &'a Path,
    pub pipeline_dir: Option<&'a Path>,
    pub policy: PlanPolicy<'a>,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub output: &'a Path,
    pub arch: &'a str,
    pub runner: &'a str,
    pub source_date_epoch: Option<u32>,
    pub jobs: u32,
    pub step_timeout: u64,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub unsafe_host_root: bool,
    pub quiet: bool,
    pub json: bool,
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
