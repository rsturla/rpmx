use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "rpmx", version, about = "Build RPMs from package.yaml")]
pub struct Cli {
    /// Directory containing reusable step files (default: <recipe>/pipelines).
    #[arg(long, global = true, value_name = "DIR")]
    pub pipeline_dir: Option<PathBuf>,
    /// Keep temporary work directories and print their paths.
    #[arg(long, global = true)]
    pub keep_workdir: bool,
    /// Echo step commands before they run.
    #[arg(long, global = true)]
    pub debug: bool,
    /// List resolved detail such as every build-root package.
    #[arg(long, global = true)]
    pub verbose: bool,
    /// Color progress lines: auto, always or never.
    #[arg(
        long,
        global = true,
        default_value = "auto",
        value_parser = ["auto", "always", "never"]
    )]
    pub color: String,
    /// Emit machine-readable output where the command supports it.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Stable command name for machine-readable documents.
    pub fn command_name(&self) -> &'static str {
        match &self.command {
            Command::Explain(_) => "explain",
            Command::Shell(_) => "shell",
            Command::Steps { .. } => "steps",
            Command::Doctor(_) => "doctor",
            Command::Bump(_) => "bump",
            Command::Init(_) => "init",
            Command::Lint(_) => "lint",
            Command::Lock(_) => "lock",
            Command::Plan(_) => "plan",
            Command::Bootstrap { .. } => "bootstrap",
            Command::Root { .. } => "root",
            Command::Runner { .. } => "runner",
            Command::Fetch(_) => "fetch",
            Command::Source(_) => "source",
            Command::Srpm(_) => "srpm",
            Command::Rebuild(_) => "rebuild",
            Command::Repo(_) => "repo",
            Command::Cache { .. } => "cache",
            Command::Keygen(_) => "keygen",
            Command::Sign(_) => "sign",
            Command::Build(_) => "build",
            Command::Test(_) => "test",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum RootCommand {
    /// Resolve a profile into a pinned root lock.
    Resolve(crate::commands::RootResolveArgs),
    /// Assemble a locked root offline from cached payloads.
    Assemble(crate::commands::RootAssembleArgs),
    /// Compute the deterministic digest of an assembled root.
    Digest(crate::commands::RootDigestArgs),
    /// Export a root lock as a libpkgmanifest rpm-package-manifest.
    Export(crate::commands::RootExportArgs),
    /// Import a libpkgmanifest rpm-package-manifest as a root lock.
    Import(crate::commands::RootImportArgs),
}

#[derive(Debug, Subcommand)]
pub enum RunnerCommand {
    /// Resolve the locked runner asset (the QEMU appliance today).
    Resolve(crate::commands::ApplianceResolveArgs),
    /// Assemble a runner asset lock into the cache or an explicit directory.
    Assemble(crate::commands::ApplianceAssembleArgs),
    /// Print the runner asset closure and identity.
    Inspect(crate::commands::ApplianceInspectArgs),
}

#[derive(Debug, Subcommand)]
pub enum BootstrapCommand {
    /// Validate a manifest and print the stage graph without resolving roots.
    Plan(crate::commands::BootstrapPlanArgs),
    /// Resolve stage roots, build every stage in order, and index its repository.
    Build(crate::commands::BootstrapBuildArgs),
    /// Rebuild one stage package against a bootstrap stage lock and re-emit its SRPM.
    Rebuild(Box<crate::commands::BootstrapRebuildArgs>),
}

#[derive(Debug, Subcommand)]
pub enum StepsCommand {
    /// List every built-in step.
    List(crate::commands::StepsListArgs),
    /// Print one built-in step definition.
    Show(crate::commands::StepsShowArgs),
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Remove cached payloads not referenced by the given locks and recipes.
    Gc(crate::commands::GcArgs),
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
    /// Explain a resolved policy, root, step list, matrix or dependency set.
    Explain(crate::commands::ExplainArgs),
    /// Open a shell in the build sandbox.
    Shell(crate::commands::ShellArgs),
    /// Inspect the built-in step library.
    Steps {
        #[command(subcommand)]
        command: StepsCommand,
    },
    /// Check the host features rpmx depends on.
    Doctor(crate::commands::DoctorArgs),
    /// Update a recipe to a new upstream version.
    Bump(crate::commands::BumpArgs),
    /// Create a package that builds on the first try.
    Init(crate::commands::InitArgs),
    /// Validate a recipe and expand its matrix.
    Lint(crate::commands::LintArgs),
    /// Write or check the package lock.
    Lock(crate::commands::LockArgs),
    /// Show the build cells, outputs and tests without building.
    Plan(crate::commands::PlanArgs),
    /// Plan and build an ordered bootstrap ladder.
    Bootstrap {
        #[command(subcommand)]
        command: BootstrapCommand,
    },
    /// Resolve, assemble and export build roots.
    /// Resolve, assemble and export build roots.
    Root {
        #[command(subcommand)]
        command: RootCommand,
    },
    /// Resolve, assemble and inspect runner-owned locked assets.
    #[command(alias = "appliance")]
    Runner {
        #[command(subcommand)]
        command: RunnerCommand,
    },
    /// Populate the content-addressed cache from the recipe's locators.
    Fetch(crate::commands::FetchArgs),
    /// Fetch and assemble the source snapshot.
    Source(crate::commands::SourceArgs),
    /// Write a native source RPM.
    Srpm(crate::commands::SrpmArgs),
    /// Rebuild binary RPMs from a native source RPM.
    Rebuild(crate::commands::RebuildArgs),
    /// Write repository metadata for a directory of built RPMs.
    Repo(crate::commands::RepoArgs),
    /// Inspect and maintain the content-addressed cache.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Generate an OpenPGP key for package signing.
    Keygen(crate::commands::KeygenArgs),
    /// Sign RPM packages in place with an OpenPGP secret key.
    Sign(crate::commands::SignArgs),
    /// Accept existing binary RPMs with the recipe's installed-package tests.
    Test(crate::commands::TestArgs),
    /// Build a recipe end to end, or package an already-staged tree.
    Build(crate::commands::BuildArgs),
}
