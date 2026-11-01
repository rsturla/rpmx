use std::num::{NonZeroU32, NonZeroU64};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, clap::Args)]
pub struct OptionalSigningArgs {
    /// Sign with this ASCII-armored OpenPGP secret key.
    #[arg(long)]
    pub signing_key: Option<PathBuf>,
    /// Read the signing key passphrase from this environment variable.
    #[arg(long, requires = "signing_key")]
    pub signing_passphrase_env: Option<String>,
    /// Signature creation time in Unix seconds.
    #[arg(long, requires = "signing_key")]
    pub signing_timestamp: Option<u32>,
}

#[derive(Debug, clap::Args)]
pub struct BuildArgs {
    /// Recipe to build. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
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
    /// Output directory for binary RPMs and build-level SBOMs
    /// (default: ./packages/<arch>).
    #[arg(long, visible_alias = "out-dir")]
    pub output: Option<PathBuf>,
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
    /// Refuse to build when the recipe differs from package.lock.json.
    #[arg(long)]
    pub locked: bool,
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
    #[arg(long, default_value_t = crate::commands::default_jobs())]
    pub jobs: NonZeroU32,
    /// Wall-clock limit for each step in seconds (must be at least 1).
    #[arg(long, default_value_t = NonZeroU64::new(3600).unwrap())]
    pub step_timeout: NonZeroU64,
    /// Run build-stage and installed-stage tests.
    #[arg(long)]
    pub run_tests: bool,
    /// Do not write repository metadata after the build.
    #[arg(long)]
    pub no_index: bool,
    /// Run only the named test; repeat for several.
    #[arg(long = "test", value_name = "NAME")]
    pub test: Vec<String>,
    /// Write a JUnit XML report for the tests that ran.
    #[arg(long)]
    pub junit: Option<PathBuf>,
    /// Override the build-level SPDX SBOM path.
    #[arg(long)]
    pub sbom: Option<PathBuf>,
    /// Use a pinned test-root lock instead of resolving one.
    #[arg(long, requires = "run_tests")]
    pub test_root_lock: Option<PathBuf>,
    #[command(flatten)]
    pub signing: OptionalSigningArgs,
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
    /// Output directory (default: ./packages/<arch>).
    #[arg(long, visible_alias = "out-dir")]
    pub output: Option<PathBuf>,
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
    #[arg(long, default_value_t = crate::commands::default_jobs())]
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
    /// Content-addressed cache (default: $XDG_CACHE_HOME/rpmx/cas).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Do not use the network; fail if the root is not cached.
    #[arg(long)]
    pub offline: bool,
    /// Run build-stage tests after the build.
    #[arg(long)]
    pub run_tests: bool,
    /// Do not write repository metadata after the build.
    #[arg(long)]
    pub no_index: bool,
    /// Run only the named test; repeat for several.
    #[arg(long = "test", value_name = "NAME")]
    pub test: Vec<String>,
    /// Write a JUnit XML report for the tests that ran.
    #[arg(long)]
    pub junit: Option<PathBuf>,
    /// Override the build-level SPDX SBOM path.
    #[arg(long)]
    pub sbom: Option<PathBuf>,
    /// Use a pinned test-root lock instead of resolving one.
    #[arg(long, requires = "run_tests")]
    pub test_root_lock: Option<PathBuf>,
    #[command(flatten)]
    pub signing: OptionalSigningArgs,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
    /// Suppress reporting when this command is used as a pipeline step.
    #[arg(skip)]
    pub quiet: bool,
}

#[derive(Debug, clap::Args)]
pub struct LintArgs {
    /// Recipe to check. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    /// Target architecture used for templates.
    #[arg(long, default_value = rpmx_core::arch::native_arch())]
    pub arch: String,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Treat warnings as errors.
    #[arg(long)]
    pub strict: bool,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct ExplainArgs {
    /// What to explain.
    #[arg(value_parser = ["policy", "root", "steps", "matrix", "requires", "files"])]
    pub topic: String,
    /// Recipe to explain (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
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
pub struct StepsListArgs {
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct StepsShowArgs {
    /// Step name, such as cmake/configure.
    pub name: String,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
    /// Exit non-zero when a check fails.
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Directory to create (default: the current directory).
    pub directory: Option<PathBuf>,
    /// Package template.
    #[arg(long, default_value = "c", value_parser = ["c", "python", "metapackage"])]
    pub template: String,
    /// Profile name written to profiles/<name>.yaml.
    #[arg(long, default_value = "fedora-44")]
    pub profile: String,
    /// Overwrite files that already exist.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, clap::Args)]
pub struct TestArgs {
    /// Recipe to test (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Target architecture used for templates.
    #[arg(long, default_value = rpmx_core::arch::native_arch())]
    pub arch: String,
    /// Runner: auto, bwrap or qemu.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bwrap", "qemu"])]
    pub runner: String,
    /// Run only the named test; repeat for several.
    #[arg(long = "test", value_name = "NAME")]
    pub selected: Vec<String>,
    /// Test an existing directory of RPMs instead of building first.
    #[arg(long)]
    pub install_dir: Option<PathBuf>,
    /// Write a JUnit XML report.
    #[arg(long)]
    pub junit: Option<PathBuf>,
    /// Root lock for the test environment.
    #[arg(long)]
    pub root_lock: Option<PathBuf>,
    /// Assembled root for the test environment.
    #[arg(long, requires = "root_lock")]
    pub root: Option<PathBuf>,
    /// Content-addressed cache (default: $XDG_CACHE_HOME/rpmx/cas).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Do not use the network; fail on a cache miss.
    #[arg(long)]
    pub offline: bool,
    /// Parallelism exposed as JOBS.
    #[arg(long, default_value_t = crate::commands::default_jobs())]
    pub jobs: NonZeroU32,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct LockArgs {
    /// Recipe to lock (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    /// Target architecture used for templates.
    #[arg(long, default_value = rpmx_core::arch::native_arch())]
    pub arch: String,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Verify the lock without writing it; exit 3 on drift.
    #[arg(long)]
    pub check: bool,
    /// Write the lock here (default: package.lock.json next to the recipe).
    #[arg(long, short)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
pub struct PlanArgs {
    /// Recipe to plan. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
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
    /// Recipe whose sources should be fetched. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
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
    /// Recipe to assemble. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
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
    /// Refuse to build when the recipe differs from package.lock.json.
    #[arg(long)]
    pub locked: bool,
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
    #[arg(long, default_value_t = crate::commands::default_jobs())]
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
    /// Recipe to package. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
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
    #[command(flatten)]
    pub signing: OptionalSigningArgs,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
    /// Suppress reporting when this command is used as a pipeline step.
    #[arg(skip)]
    pub quiet: bool,
}

#[derive(Debug, clap::Args)]
pub struct RootResolveArgs {
    /// Profile file.
    pub profile: PathBuf,
    /// Write the root lock here (default: standard output).
    #[arg(long, short)]
    pub output: Option<PathBuf>,
    /// Resolver scratch directory (default: a temporary directory).
    #[arg(long)]
    pub work: Option<PathBuf>,
    /// Keep the resolver scratch directory.
    #[arg(long)]
    pub keep: bool,
    /// Content-addressed cache (default: $XDG_CACHE_HOME/rpmx/cas).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct RootAssembleArgs {
    /// Root lock to assemble.
    pub lock: PathBuf,
    /// Directory to assemble the root in (default: <output>/../root).
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Write the lock with root_digest here (default: standard output).
    #[arg(long, short)]
    pub output: Option<PathBuf>,
    /// Content-addressed cache (default: $XDG_CACHE_HOME/rpmx/cas).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Assembly scratch directory (default: a temporary directory).
    #[arg(long)]
    pub work: Option<PathBuf>,
    /// Keep the assembly scratch directory.
    #[arg(long)]
    pub keep: bool,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct RootDigestArgs {
    /// Assembled root directory.
    pub root: PathBuf,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct RootExportArgs {
    /// Root lock file.
    pub lock: PathBuf,
    /// Write the manifest here (default: standard output).
    #[arg(long, short)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
pub struct RootImportArgs {
    /// Manifest file.
    pub manifest: PathBuf,
    /// Profile name recorded in the lock.
    #[arg(long, default_value = "imported")]
    pub profile: String,
    /// Target architecture when the manifest has no unambiguous one.
    #[arg(long)]
    pub arch: Option<String>,
    /// Release version recorded in the lock.
    #[arg(long, default_value = "")]
    pub releasever: String,
    /// Write the root lock here (default: standard output).
    #[arg(long, short)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
pub struct RepoArgs {
    /// Directory containing the RPMs.
    #[arg(long)]
    pub rpms: PathBuf,
    /// Repository directory; RPMs are copied here and repodata is written.
    #[arg(long)]
    pub output: PathBuf,
    #[command(flatten)]
    pub signing: OptionalSigningArgs,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct KeygenArgs {
    /// Secret key output path; the public key is written beside it with a
    /// `.pub` suffix.
    #[arg(long, short, default_value = "rpmx-signing-key.asc")]
    pub output: PathBuf,
    /// OpenPGP user ID for the key.
    #[arg(long, default_value = "rpmx signing key <rpmx@localhost>")]
    pub user_id: String,
    /// RSA key size in bits.
    #[arg(long, default_value_t = 4096)]
    pub bits: u32,
    /// Read the key passphrase from this environment variable.
    #[arg(long)]
    pub passphrase_env: Option<String>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct SignArgs {
    /// ASCII-armored OpenPGP secret key.
    #[arg(long, short = 'k')]
    pub signing_key: PathBuf,
    /// Read the key passphrase from this environment variable.
    #[arg(long)]
    pub passphrase_env: Option<String>,
    /// Signature creation time in Unix seconds (default: now).
    #[arg(long)]
    pub timestamp: Option<u32>,
    /// RPM files to sign.
    #[arg(required = true)]
    pub rpms: Vec<PathBuf>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct GcArgs {
    /// Root lock whose packages must stay cached.
    #[arg(long)]
    pub lock: Vec<PathBuf>,
    /// Recipe whose sources and tools must stay cached.
    #[arg(long)]
    pub recipe: Vec<PathBuf>,
    /// Keep blobs younger than this many hours, protecting concurrent fetches.
    #[arg(long, default_value_t = 24)]
    pub older_than_hours: u64,
    /// Report what would be removed without deleting it.
    #[arg(long)]
    pub dry_run: bool,
    /// CAS directory (default: $XDG_CACHE_HOME/rpmx/cas).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Emit machine-readable output.
    #[arg(long)]
    pub json: bool,
}
