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

/// Cache selection shared by every command that reads or writes the CAS.
#[derive(Debug, Clone, clap::Args)]
pub struct CacheArgs {
    /// Content-addressed cache (default: $XDG_CACHE_HOME/rpmx/cas).
    #[arg(long, value_name = "DIR")]
    pub cache: Option<PathBuf>,
}

/// Execution knobs shared by every command that runs build steps.
#[derive(Debug, Clone, clap::Args)]
pub struct RuntimeArgs {
    #[command(flatten)]
    pub cache: CacheArgs,
    /// Parallelism exposed to build steps as JOBS.
    #[arg(long, default_value_t = crate::commands::default_jobs())]
    pub jobs: NonZeroU32,
    /// Wall-clock limit for each step in seconds (must be at least 1).
    #[arg(long, default_value_t = NonZeroU64::new(3600).unwrap())]
    pub step_timeout: NonZeroU64,
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
    /// Artifacts root; each architecture writes to ROOT/<arch>
    /// (default: ./packages/<arch>).
    #[arg(long, visible_alias = "out-dir")]
    pub output: Option<PathBuf>,
    /// Target architectures (repeatable or comma-separated; default: native).
    #[arg(long, value_delimiter = ',')]
    pub arch: Vec<String>,
    /// Concurrent architecture builds: a number, or "all" (default: auto).
    #[arg(long, value_name = "N|all")]
    pub arch_jobs: Option<String>,
    /// Runner: auto, bwrap, podman or qemu.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bwrap", "podman", "qemu"])]
    pub runner: String,
    /// Reproducible timestamp in Unix seconds (default: package.source-date-epoch).
    #[arg(long)]
    pub source_date_epoch: Option<u32>,
    /// Build one matrix cell.
    #[arg(long)]
    pub cell: Option<String>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
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
    /// Use this assembled QEMU appliance instead of the cache.
    #[arg(long, visible_alias = "appliance-root", value_name = "DIR")]
    pub runner_asset_root: Option<PathBuf>,
    /// Use the host root for source assembly (unsafe).
    #[arg(long)]
    pub unsafe_host_root: bool,
    /// Run the recipe's tests (the default; kept for explicitness).
    #[arg(long)]
    pub run_tests: bool,
    /// Do not run any tests.
    #[arg(long, conflicts_with_all = ["run_tests", "test"])]
    pub no_tests: bool,
    /// With tests, skip installed-stage tests (they execute target binaries).
    #[arg(long)]
    pub skip_installed: bool,
    /// Keep the prepared installed-stage test root here so it can be shipped.
    #[arg(long, value_name = "DIR")]
    pub save_test_root: Option<PathBuf>,
    /// Do not write repository metadata after the build.
    #[arg(long)]
    pub no_index: bool,
    /// Skip package policy checks.
    #[arg(long)]
    pub no_lint: bool,
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
    #[arg(long, conflicts_with = "no_tests")]
    pub test_root_lock: Option<PathBuf>,
    #[command(flatten)]
    pub signing: OptionalSigningArgs,
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
    /// Architecture for outputs that do not declare one. Defaults to the
    /// architecture recorded in the SRPM.
    #[arg(long)]
    pub arch: Option<String>,
    /// Runner: auto, bwrap, podman or qemu.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bwrap", "podman", "qemu"])]
    pub runner: String,
    /// Build one matrix cell (required when the plan has several).
    #[arg(long)]
    pub cell: Option<String>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    /// Run steps inside this builder image instead of host bwrap.
    #[arg(long, conflicts_with = "root_lock")]
    pub root_image: Option<String>,
    /// Root lock describing the build root.
    #[arg(long)]
    pub root_lock: Option<PathBuf>,
    /// Assembled root directory.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Use this assembled QEMU appliance instead of the cache.
    #[arg(long, visible_alias = "appliance-root", value_name = "DIR")]
    pub runner_asset_root: Option<PathBuf>,
    /// Do not use the network; fail if the root is not cached.
    #[arg(long)]
    pub offline: bool,
    /// Run the recipe's tests (the default; kept for explicitness).
    #[arg(long)]
    pub run_tests: bool,
    /// Do not run any tests.
    #[arg(long, conflicts_with_all = ["run_tests", "test"])]
    pub no_tests: bool,
    /// With tests, skip installed-stage tests (they execute target binaries).
    #[arg(long)]
    pub skip_installed: bool,
    /// Keep the prepared installed-stage test root here so it can be shipped.
    #[arg(long, value_name = "DIR")]
    pub save_test_root: Option<PathBuf>,
    /// Do not write repository metadata after the build.
    #[arg(long)]
    pub no_index: bool,
    /// Skip package policy checks.
    #[arg(long)]
    pub no_lint: bool,
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
    #[arg(long, conflicts_with = "no_tests")]
    pub test_root_lock: Option<PathBuf>,
    #[command(flatten)]
    pub signing: OptionalSigningArgs,
    /// Suppress reporting when this command is used as a pipeline step.
    #[arg(skip)]
    pub quiet: bool,
}

#[derive(Debug, clap::Args)]
pub struct LintArgs {
    /// Recipe to check. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    /// Target architecture used for templates. Defaults to the single target
    /// the recipe's profile declares, or the host architecture.
    #[arg(long)]
    pub arch: Option<String>,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Treat warnings as errors.
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, clap::Args)]
pub struct BumpArgs {
    /// New upstream version.
    pub version: String,
    /// Recipe to update (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    /// Print the diff without writing the recipe.
    #[arg(long)]
    pub dry_run: bool,
    /// Do not download sources to refresh their digests.
    #[arg(long)]
    pub no_fetch: bool,
}

#[derive(Debug, clap::Args)]
pub struct ShellArgs {
    /// Recipe whose build root to enter (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Target architecture used for templates. Defaults to the single target
    /// the recipe's profile declares, or the host architecture.
    #[arg(long)]
    pub arch: Option<String>,
    /// Assembled root to use as /. Resolved from the recipe when omitted.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Host workspace, mounted at /tmp/build (default: the current directory).
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    /// Host stage, mounted at /tmp/stage (default: a temporary directory).
    #[arg(long)]
    pub stage: Option<PathBuf>,
    #[command(flatten)]
    pub cache: CacheArgs,
    /// Do not use the network; fail if the root is not cached.
    #[arg(long)]
    pub offline: bool,
    /// Run this command instead of an interactive shell.
    #[arg(long, value_name = "COMMAND")]
    pub command: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ExplainArgs {
    /// What to explain.
    #[arg(value_parser = ["policy", "root", "steps", "matrix", "requires", "files", "components"])]
    pub topic: String,
    /// Recipe to explain (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    /// Source workspace to inspect for `components`. Without it, only
    /// declarations from the recipe are shown.
    #[arg(long, value_name = "DIR")]
    pub source: Option<PathBuf>,
    /// Target architecture used for templates. Defaults to the single target
    /// the recipe's profile declares, or the host architecture.
    #[arg(long)]
    pub arch: Option<String>,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
}

#[derive(Debug, clap::Args)]
pub struct StepsListArgs {}

#[derive(Debug, clap::Args)]
pub struct StepsShowArgs {
    /// Step name, such as cmake/configure.
    pub name: String,
}

#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Also report whether this recipe's build root and cross sysroot are
    /// cached, so an offline build will work.
    #[arg(long, value_name = "FILE")]
    pub recipe: Option<PathBuf>,
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
    /// Write one file per top-level section and compose them with `include:`.
    #[arg(long)]
    pub split: bool,
}

#[derive(Debug, clap::Args)]
pub struct TestArgs {
    /// Recipe to test (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Target architecture used for templates. Defaults to the single target
    /// the recipe's profile declares, or the host architecture.
    #[arg(long)]
    pub arch: Option<String>,
    /// Runner: auto, bwrap, podman or qemu.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bwrap", "podman", "qemu"])]
    pub runner: String,
    /// Run only the named test; repeat for several.
    #[arg(long = "test", value_name = "NAME")]
    pub selected: Vec<String>,
    /// Test RPMs in this directory instead of building the recipe.
    #[arg(long, value_name = "DIR")]
    pub install_dir: Option<PathBuf>,
    /// Skip installed-stage tests (they execute target binaries).
    #[arg(long)]
    pub skip_installed: bool,
    /// Keep the prepared installed-stage test root here so it can be shipped.
    #[arg(long, value_name = "DIR")]
    pub save_test_root: Option<PathBuf>,
    /// Run the tests against this pre-assembled test root instead of preparing
    /// one. `root.lock.yaml` is read from it unless --test-root-lock is given.
    #[arg(long, value_name = "DIR")]
    pub test_root: Option<PathBuf>,
    /// Write a JUnit XML report.
    #[arg(long)]
    pub junit: Option<PathBuf>,
    /// Root lock for the test environment.
    #[arg(long)]
    pub root_lock: Option<PathBuf>,
    /// Pin the test environment to this lock instead of resolving one.
    #[arg(long, value_name = "FILE")]
    pub test_root_lock: Option<PathBuf>,
    /// Assembled root for the test environment.
    #[arg(long, requires = "root_lock")]
    pub root: Option<PathBuf>,
    /// Use this assembled QEMU appliance instead of the cache.
    #[arg(long, visible_alias = "appliance-root", value_name = "DIR")]
    pub runner_asset_root: Option<PathBuf>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    /// Do not use the network; fail on a cache miss.
    #[arg(long)]
    pub offline: bool,
}

#[derive(Debug, clap::Args)]
pub struct LockArgs {
    /// Recipe to lock (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    /// Target architecture used for templates. Defaults to the single target
    /// the recipe's profile declares, or the host architecture.
    #[arg(long)]
    pub arch: Option<String>,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    /// Verify the lock without writing it; exit 3 on drift.
    #[arg(long)]
    pub check: bool,
    /// Root lock to record the resolved closure from.
    #[arg(long)]
    pub root_lock: Option<PathBuf>,
    /// Write the lock here (default: package.lock.json next to the recipe).
    #[arg(long, short)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
pub struct PlanArgs {
    /// Recipe to plan. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    /// Target architecture used for templates. Defaults to the single target
    /// the recipe's profile declares, or the host architecture.
    #[arg(long)]
    pub arch: Option<String>,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
}

#[derive(Debug, clap::Args)]
pub struct FetchArgs {
    /// Recipe whose sources should be fetched. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    #[command(flatten)]
    pub cache: CacheArgs,
    /// Content-addressed acquisition proxy.
    #[arg(long)]
    pub proxy: Option<String>,
    /// Do not use the network; fail on a cache miss.
    #[arg(long)]
    pub offline: bool,
}

#[derive(Debug, clap::Args)]
pub struct SourceArgs {
    /// Recipe to assemble. (default: ./package.yaml).
    #[arg(default_value = "package.yaml")]
    pub file: PathBuf,
    /// Target architecture used for templates. Defaults to the single target
    /// the recipe's profile declares, or the host architecture.
    #[arg(long)]
    pub arch: Option<String>,
    /// Runner: auto, bwrap, podman or qemu.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bwrap", "podman", "qemu"])]
    pub runner: String,
    #[command(flatten)]
    pub policy: crate::cli::PolicyArgs,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
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
    /// Use this assembled QEMU appliance instead of the cache.
    #[arg(long, visible_alias = "appliance-root", value_name = "DIR")]
    pub runner_asset_root: Option<PathBuf>,
    /// Use the host root for source assembly (unsafe).
    #[arg(long)]
    pub unsafe_host_root: bool,
    /// Output directory for RPMX-SOURCE.tar.gz.
    #[arg(long, default_value = ".")]
    pub output: PathBuf,
    /// Reproducible timestamp in Unix seconds (default: package.source-date-epoch).
    #[arg(long)]
    pub source_date_epoch: Option<u32>,
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
    /// Directory containing a prebuilt source snapshot; omit to assemble one.
    #[arg(long)]
    pub sources: Option<PathBuf>,
    /// Output directory.
    #[arg(long, default_value = ".")]
    pub output: PathBuf,
    /// Build architecture recorded in the source header. Defaults to the
    /// single target the recipe's profile declares, or the host architecture.
    #[arg(long)]
    pub arch: Option<String>,
    /// Runner: auto, bwrap, podman or qemu.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bwrap", "podman", "qemu"])]
    pub runner: String,
    /// Reproducible timestamp in Unix seconds (default: package.source-date-epoch).
    #[arg(long)]
    pub source_date_epoch: Option<u32>,
    /// Write one matrix cell (default: every cell).
    #[arg(long)]
    pub cell: Option<String>,
    /// Root lock to embed in the SRPM.
    #[arg(long)]
    pub root_lock: Option<PathBuf>,
    /// Assembled root for source assembly.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Use the host root for source assembly (unsafe).
    #[arg(long)]
    pub unsafe_host_root: bool,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    /// Content-addressed acquisition proxy.
    #[arg(long)]
    pub proxy: Option<String>,
    /// Do not use the network; fail on a cache miss.
    #[arg(long)]
    pub offline: bool,
    #[command(flatten)]
    pub signing: OptionalSigningArgs,
    /// Suppress reporting when this command is used as a pipeline step.
    #[arg(skip)]
    pub quiet: bool,
}

#[derive(Debug, clap::Args)]
pub struct RootResolveArgs {
    /// Profile file.
    pub profile: PathBuf,
    /// Target architecture (default: the profile's only arch).
    #[arg(long)]
    pub arch: Option<String>,
    /// Write the root lock here (default: standard output).
    #[arg(long, short)]
    pub output: Option<PathBuf>,
    /// Resolver scratch directory (default: a temporary directory).
    #[arg(long)]
    pub work: Option<PathBuf>,
    /// Keep the resolver scratch directory.
    #[arg(long)]
    pub keep: bool,
    #[command(flatten)]
    pub cache: CacheArgs,
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
    #[command(flatten)]
    pub cache: CacheArgs,
    /// Assembly scratch directory (default: a temporary directory).
    #[arg(long)]
    pub work: Option<PathBuf>,
    /// Keep the assembly scratch directory.
    #[arg(long)]
    pub keep: bool,
}

#[derive(Debug, clap::Args)]
pub struct ApplianceResolveArgs {
    /// Profile supplying repositories, releasever and the builder image.
    pub profile: PathBuf,
    /// Target architecture (default: the profile's only arch).
    #[arg(long)]
    pub arch: Option<String>,
    /// Write the standalone appliance lock here (default: standard output).
    #[arg(long, short)]
    pub output: Option<PathBuf>,
    /// Resolver scratch directory (default: a temporary directory).
    #[arg(long)]
    pub work: Option<PathBuf>,
    /// Keep the resolver scratch directory.
    #[arg(long)]
    pub keep: bool,
    #[command(flatten)]
    pub cache: CacheArgs,
}

#[derive(Debug, clap::Args)]
pub struct ApplianceAssembleArgs {
    /// Standalone appliance lock written by `rpmx appliance resolve`.
    pub lock: PathBuf,
    /// Assemble here instead of the content-addressed cache.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Do not use the network; fail if the appliance is not cached.
    #[arg(long)]
    pub offline: bool,
    #[command(flatten)]
    pub cache: CacheArgs,
}

#[derive(Debug, clap::Args)]
pub struct ApplianceInspectArgs {
    /// Standalone runner asset lock.
    #[arg(long, alias = "appliance-lock")]
    pub asset_lock: PathBuf,
}

#[derive(Debug, clap::Args)]
pub struct RootDigestArgs {
    /// Assembled root directory.
    pub root: PathBuf,
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
    #[command(flatten)]
    pub cache: CacheArgs,
}
