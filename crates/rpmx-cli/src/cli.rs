use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "rpmx", version, about = "Build RPMs from package.yaml")]
pub struct Cli {
    /// Directory containing reusable step files (default: <recipe>/pipelines).
    #[arg(long, global = true, value_name = "DIR")]
    pub pipeline_dir: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    pub fn json(&self) -> bool {
        match &self.command {
            Command::Root { command } => command.json(),
            Command::Lint { json, .. }
            | Command::Plan { json, .. }
            | Command::Fetch { json, .. }
            | Command::Keygen { json, .. }
            | Command::Sign { json, .. }
            | Command::Repo { json, .. } => *json,
            Command::Build(args) => args.json,
            Command::Rebuild(args) => args.json,
            Command::Source(args) => args.json,
            Command::Srpm(args) => args.json,
            Command::Cache { command } => command.json(),
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum RootCommand {
    /// Resolve a profile into a pinned root lock.
    Resolve {
        /// Profile file.
        profile: PathBuf,
        /// Write the root lock here (default: standard output).
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Resolver scratch directory (default: a temporary directory).
        #[arg(long)]
        work: Option<PathBuf>,
        /// Keep the resolver scratch directory.
        #[arg(long)]
        keep: bool,
        /// Content-addressed cache (default: $XDG_CACHE_HOME/rpmx/cas).
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Assemble a locked root offline from cached payloads.
    Assemble {
        /// Root lock to assemble.
        lock: PathBuf,
        /// Directory to assemble the root in (default: <output>/../root).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Write the lock with root_digest here (default: standard output).
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Content-addressed cache (default: $XDG_CACHE_HOME/rpmx/cas).
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Assembly scratch directory (default: a temporary directory).
        #[arg(long)]
        work: Option<PathBuf>,
        /// Keep the assembly scratch directory.
        #[arg(long)]
        keep: bool,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Compute the deterministic digest of an assembled root.
    Digest {
        /// Assembled root directory.
        root: PathBuf,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Export a root lock as a libpkgmanifest rpm-package-manifest.
    Export {
        /// Root lock file.
        lock: PathBuf,
        /// Write the manifest here (default: standard output).
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Import a libpkgmanifest rpm-package-manifest as a root lock.
    Import {
        /// Manifest file.
        manifest: PathBuf,
        /// Profile name recorded in the lock.
        #[arg(long, default_value = "imported")]
        profile: String,
        /// Target architecture when the manifest has no unambiguous one.
        #[arg(long)]
        arch: Option<String>,
        /// Release version recorded in the lock.
        #[arg(long, default_value = "")]
        releasever: String,
        /// Write the root lock here (default: standard output).
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
}

impl RootCommand {
    pub fn json(&self) -> bool {
        match self {
            RootCommand::Resolve { json, .. }
            | RootCommand::Assemble { json, .. }
            | RootCommand::Digest { json, .. } => *json,
            RootCommand::Export { .. } | RootCommand::Import { .. } => false,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Remove cached payloads not referenced by the given locks and recipes.
    Gc {
        /// Root lock whose packages must stay cached.
        #[arg(long)]
        lock: Vec<PathBuf>,
        /// Recipe whose sources and tools must stay cached.
        #[arg(long)]
        recipe: Vec<PathBuf>,
        /// Keep blobs younger than this many hours, protecting concurrent fetches.
        #[arg(long, default_value_t = 24)]
        older_than_hours: u64,
        /// Report what would be removed without deleting it.
        #[arg(long)]
        dry_run: bool,
        /// CAS directory (default: $XDG_CACHE_HOME/rpmx/cas).
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

impl CacheCommand {
    pub fn json(&self) -> bool {
        match self {
            CacheCommand::Gc { json, .. } => *json,
        }
    }
}

/// Distribution policy shared by the commands that plan a recipe.
#[derive(Debug, clap::Args)]
pub struct PolicyArgs {
    /// Build profile supplying policy defaults such as the dist tag.
    #[arg(long, value_name = "FILE")]
    pub profile: Option<PathBuf>,
    /// Distribution tag appended to every release, such as `fc44` or `.el9_2`.
    #[arg(long, value_name = "TAG")]
    pub dist_tag: Option<String>,
    /// Build-system counter rendered as `+build<N>` after the dist tag.
    #[arg(long, value_name = "N")]
    pub build_release: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Validate a recipe and expand its matrix.
    Lint {
        /// Recipe to check.
        file: PathBuf,
        /// Target architecture used for templates.
        #[arg(long, default_value = rpmx_core::arch::native_arch())]
        arch: String,
        #[command(flatten)]
        policy: PolicyArgs,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Show the build cells, outputs and tests without building.
    Plan {
        /// Recipe to plan.
        file: PathBuf,
        /// Target architecture used for templates.
        #[arg(long, default_value = rpmx_core::arch::native_arch())]
        arch: String,
        #[command(flatten)]
        policy: PolicyArgs,
        /// Emit the plan as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Resolve, assemble and export build roots.
    Root {
        #[command(subcommand)]
        command: RootCommand,
    },
    /// Populate the content-addressed cache from the recipe's locators.
    Fetch {
        /// Recipe whose sources should be fetched.
        file: PathBuf,
        /// CAS directory (default: $XDG_CACHE_HOME/rpmx/cas).
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Content-addressed acquisition proxy.
        #[arg(long)]
        proxy: Option<String>,
        /// Do not use the network; fail on a cache miss.
        #[arg(long)]
        offline: bool,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Fetch and assemble the source snapshot.
    Source(crate::commands::SourceArgs),
    /// Write a native source RPM from a local source snapshot.
    Srpm(crate::commands::SrpmArgs),
    /// Rebuild binary RPMs from a native source RPM.
    Rebuild(crate::commands::RebuildArgs),
    /// Write repository metadata for a directory of built RPMs.
    Repo {
        /// Directory containing the RPMs.
        #[arg(long)]
        rpms: PathBuf,
        /// Repository directory; RPMs are copied here and repodata is written.
        #[arg(long)]
        output: PathBuf,
        /// Sign repodata with this ASCII-armored OpenPGP secret key.
        #[arg(long)]
        signing_key: Option<PathBuf>,
        /// Read the signing key passphrase from this environment variable.
        #[arg(long, requires = "signing_key")]
        signing_passphrase_env: Option<String>,
        /// Repository revision/signature timestamp in Unix seconds (default: now).
        #[arg(long, requires = "signing_key")]
        signing_timestamp: Option<u32>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Inspect and maintain the content-addressed cache.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Generate an OpenPGP key for package signing.
    ///
    /// rpmx does not hold keys. The generated secret key is written where you
    /// ask for it; pass it to `rpmx sign` or `rpmx rebuild --signing-key`.
    Keygen {
        /// Secret key output path; the public key is written beside it with a
        /// `.pub` suffix.
        #[arg(long, short, default_value = "rpmx-signing-key.asc")]
        output: PathBuf,
        /// OpenPGP user ID for the key.
        #[arg(long, default_value = "rpmx signing key <rpmx@localhost>")]
        user_id: String,
        /// RSA key size in bits.
        #[arg(long, default_value_t = 4096)]
        bits: u32,
        /// Read the key passphrase from this environment variable.
        #[arg(long)]
        passphrase_env: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Sign RPM packages in place with an OpenPGP secret key.
    Sign {
        /// ASCII-armored OpenPGP secret key.
        #[arg(long, short = 'k')]
        signing_key: PathBuf,
        /// Read the key passphrase from this environment variable.
        #[arg(long)]
        passphrase_env: Option<String>,
        /// Signature creation time in Unix seconds (default: now).
        #[arg(long)]
        timestamp: Option<u32>,
        /// RPM files to sign.
        #[arg(required = true)]
        rpms: Vec<PathBuf>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Build a recipe end to end, or package an already-staged tree.
    Build(crate::commands::BuildArgs),
}
