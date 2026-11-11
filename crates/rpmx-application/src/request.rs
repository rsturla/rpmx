use std::num::{NonZeroU32, NonZeroU64};
use std::path::PathBuf;

/// Choices made by the caller. Execution architecture, sysroots, builder
/// identities and resolved closures are derived by the application.
#[derive(Debug, Clone)]
pub struct ApplicationConfig {
    pub cache: Option<PathBuf>,
    pub offline: bool,
    pub runner: String,
    pub jobs: NonZeroU32,
    pub timeout: NonZeroU64,
    pub pipelines: Option<PathBuf>,
    pub root: BuildRoot,
    pub policy: ProductionPolicy,
    pub signing: SigningPolicy,
    pub proxy: Option<String>,
    pub runner_assets: Option<PathBuf>,
    pub quiet: bool,
}

impl Default for ApplicationConfig {
    fn default() -> Self {
        Self {
            cache: None,
            offline: false,
            runner: "auto".into(),
            jobs: rpmx_base::output::default_jobs(),
            timeout: NonZeroU64::new(3600).unwrap(),
            pipelines: None,
            root: BuildRoot::Automatic { profile: None },
            policy: ProductionPolicy::default(),
            signing: SigningPolicy::default(),
            proxy: None,
            runner_assets: None,
            quiet: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum BuildRoot {
    Automatic {
        profile: Option<PathBuf>,
    },
    Locked {
        lock: PathBuf,
        root: Option<PathBuf>,
    },
    UnsafeHost,
    Bootstrap(BootstrapEnvironment),
}

#[derive(Debug, Clone)]
pub struct SavedRoot {
    pub lock: PathBuf,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum BootstrapEnvironment {
    Native {
        build: SavedRoot,
    },
    Cross {
        build: SavedRoot,
        target: SavedRoot,
        toolchain: Box<crate::domain::guest::CrossToolchain>,
    },
}

impl Default for BuildRoot {
    fn default() -> Self {
        Self::Automatic { profile: None }
    }
}

impl BuildRoot {
    pub(crate) fn profile(&self) -> Option<&std::path::Path> {
        match self {
            Self::Automatic { profile } => profile.as_deref(),
            _ => None,
        }
    }
    pub(crate) fn lock(&self) -> Option<&std::path::Path> {
        match self {
            Self::Locked { lock, .. } => Some(lock),
            _ => None,
        }
    }
    pub(crate) fn path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Locked { root, .. } => root.as_deref(),
            _ => None,
        }
    }
    pub(crate) fn unsafe_host(&self) -> bool {
        matches!(self, Self::UnsafeHost)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProductionPolicy {
    pub dist_tag: Option<String>,
    pub build_release: Option<u32>,
    pub source_date_epoch: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct SigningPolicy {
    pub signing_key: Option<PathBuf>,
    pub signing_passphrase_env: Option<String>,
    pub signing_timestamp: Option<u32>,
}

pub struct ApplicationRequest {
    pub operation: Operation,
    pub output: PathBuf,
}

pub enum Operation {
    Source(SourceInput),
    Srpm(SrpmInput),
    Build(BuildInput),
    Rebuild(RebuildInput),
    Accept(crate::acceptance::AcceptInput),
    Debug(DebugInput),
}

#[derive(Debug, Clone)]
pub struct SourceInput {
    pub recipe: PathBuf,
    pub arch: Option<String>,
    pub locked: bool,
}

#[derive(Debug, Clone)]
pub struct SrpmInput {
    pub recipe: PathBuf,
    pub arch: Option<String>,
    pub sources: Option<PathBuf>,
    pub cell: Option<String>,
}

#[derive(Debug, Clone)]
pub enum BuildOutputLayout {
    Architectures,
    Flat,
}

#[derive(Debug, Clone)]
pub struct BuildInput {
    pub recipe: PathBuf,
    pub architectures: Vec<String>,
    pub layout: BuildOutputLayout,
    pub arch_jobs: Option<String>,
    pub cell: Option<String>,
    pub stage: Option<PathBuf>,
    pub build_tests: bool,
    pub locked: bool,
    pub lint: bool,
    pub index: bool,
    pub sbom: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RebuildInput {
    pub srpm: PathBuf,
    pub arch: Option<String>,
    pub cell: Option<String>,
    pub build_tests: bool,
    pub lint: bool,
    pub index: bool,
    pub sbom: Option<PathBuf>,
    /// Explicit bootstrap replacement is a different production operation;
    /// ordinary rebuilds cannot silently replace embedded build provenance.
    pub replace_root_lock: bool,
    pub replacement_srpm: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct DebugInput {
    pub recipe: PathBuf,
    pub arch: Option<String>,
    pub workspace: Option<PathBuf>,
    pub stage: Option<PathBuf>,
    pub command: Option<String>,
}

impl BuildInput {
    pub fn new(recipe: impl Into<PathBuf>) -> Self {
        Self {
            recipe: recipe.into(),
            architectures: Vec::new(),
            layout: BuildOutputLayout::Architectures,
            arch_jobs: None,
            cell: None,
            stage: None,
            build_tests: true,
            locked: false,
            lint: true,
            index: true,
            sbom: None,
        }
    }
}
impl ApplicationRequest {
    pub fn build(recipe: impl Into<PathBuf>, output: impl Into<PathBuf>) -> Self {
        Self {
            operation: Operation::Build(BuildInput::new(recipe)),
            output: output.into(),
        }
    }
    pub fn rebuild(srpm: impl Into<PathBuf>, output: impl Into<PathBuf>) -> Self {
        Self {
            operation: Operation::Rebuild(RebuildInput {
                srpm: srpm.into(),
                arch: None,
                cell: None,
                build_tests: true,
                lint: true,
                index: true,
                sbom: None,
                replace_root_lock: false,
                replacement_srpm: None,
            }),
            output: output.into(),
        }
    }
}
