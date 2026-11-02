use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::Path;

use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};

mod backend;

use backend::{BwrapBackend, PodmanBackend, QemuBackend};

/// A runner-owned artifact that is locked separately from the build root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunnerAssetKind {
    /// The QEMU transport appliance: kernel, initramfs and SSH tooling.
    QemuAppliance,
}

impl RunnerAssetKind {
    pub const ALL: &'static [Self] = &[Self::QemuAppliance];

    pub fn name(self) -> &'static str {
        match self {
            Self::QemuAppliance => "qemu-appliance",
        }
    }

    /// Directory name under `$XDG_CACHE_HOME/rpmx/`.
    pub fn cache_name(self) -> &'static str {
        match self {
            Self::QemuAppliance => "appliances",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RunnerAsset<'a> {
    pub kind: RunnerAssetKind,
    pub path: &'a Path,
}

/// The runner-owned assets available to one session, if any.
///
/// The common QEMU appliance is a named slot so callers do not have to keep a
/// temporary slice alive. `others` is the extension point for future runners.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunnerAssets<'a> {
    qemu_appliance: Option<&'a Path>,
    others: &'a [RunnerAsset<'a>],
}

impl<'a> RunnerAssets<'a> {
    pub fn new(qemu_appliance: Option<&'a Path>, others: &'a [RunnerAsset<'a>]) -> Self {
        Self {
            qemu_appliance,
            others,
        }
    }

    pub fn qemu_appliance(&self) -> Option<&'a Path> {
        self.qemu_appliance
    }

    pub fn get(&self, kind: RunnerAssetKind) -> Option<&'a Path> {
        match kind {
            RunnerAssetKind::QemuAppliance => self.qemu_appliance,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.qemu_appliance.is_none() && self.others.is_empty()
    }
}

/// A runner's static capabilities. Adding a backend means adding one
/// descriptor, not editing doctor, cache reporting or asset plumbing.
#[derive(Clone, Copy, Debug)]
pub struct RunnerDescriptor {
    pub kind: RunnerKind,
    pub name: &'static str,
    pub tools: &'static [&'static str],
    pub assets: &'static [RunnerAssetKind],
}

pub const RUNNER_DESCRIPTORS: &[RunnerDescriptor] = &[
    RunnerDescriptor {
        kind: RunnerKind::Bwrap,
        name: "bubblewrap",
        tools: &["bwrap"],
        assets: &[],
    },
    RunnerDescriptor {
        kind: RunnerKind::Podman,
        name: "podman",
        tools: &["podman"],
        assets: &[],
    },
    RunnerDescriptor {
        kind: RunnerKind::Qemu,
        name: "qemu-system",
        tools: &["qemu-system-x86_64", "qemu-system-aarch64"],
        assets: &[RunnerAssetKind::QemuAppliance],
    },
];

/// The inputs a backend needs to start one session. Policy fields such as the
/// source date and debuginfo stay in `SandboxConfig`; the runner never sees
/// them.
pub struct RunnerConfig<'a> {
    pub host: Arch,
    pub target: Arch,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    /// Runner-owned locked artifacts. The build root is not one of them.
    pub assets: RunnerAssets<'a>,
    pub jobs: NonZeroU32,
    pub timeout: NonZeroU64,
}

pub struct RunnerRequest<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub cwd: &'a str,
    pub env: &'a BTreeMap<String, String>,
    pub label: &'a str,
}

/// Whether the host has an enabled qemu-user binfmt handler for `target`.
/// Root assembly still uses this; build steps use the QEMU system runner.
pub fn emulation_available(target: Arch) -> bool {
    let qemu = match target {
        Arch::I686 => "i386",
        other => other.as_str(),
    };
    let path = format!("/proc/sys/fs/binfmt_misc/qemu-{qemu}");
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let enabled = contents.lines().any(|line| line.trim() == "enabled");
    let interpreter = contents
        .lines()
        .find_map(|line| line.strip_prefix("interpreter "))
        .map(Path::new);
    enabled && interpreter.is_some_and(Path::exists)
}

pub trait Session {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()>;
    /// Copy host-visible outputs back from the session. This is a no-op for
    /// bind-mounted runners and a workspace transfer for the QEMU VM.
    fn sync_out(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}

/// A way to create an execution environment.
pub trait Runner {
    fn name(&self) -> &'static str;
    fn supports(&self, host: Arch, target: Arch) -> bool;
    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerKind {
    Auto,
    Bwrap,
    Podman,
    Qemu,
}

impl std::str::FromStr for RunnerKind {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "bwrap" => Ok(Self::Bwrap),
            "podman" => Ok(Self::Podman),
            "qemu" => Ok(Self::Qemu),
            other => Err(Error::new(
                codes::RUNNER_001,
                format!("unknown runner '{other}'; expected auto, bwrap, podman or qemu"),
            )),
        }
    }
}

impl RunnerKind {
    pub fn descriptors() -> &'static [RunnerDescriptor] {
        RUNNER_DESCRIPTORS
    }

    pub fn descriptor(self) -> Option<&'static RunnerDescriptor> {
        RUNNER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.kind == self)
    }
}

/// The pure selection decision. Kept separate from `resolve` so the root
/// binding can be asserted without starting a backend.
#[derive(Clone, Copy, Debug)]
enum Selection<'a> {
    Bwrap(Option<&'a Path>),
    Qemu(&'a Path),
    Podman(&'a str),
}

impl RunnerKind {
    fn choose<'a>(
        self,
        host: Arch,
        target: Arch,
        root: Option<&'a Path>,
        image: Option<&'a str>,
    ) -> Result<Selection<'a>> {
        match self {
            Self::Qemu => {
                let root = root.ok_or_else(|| {
                    Error::new(codes::ROOT_016, "the qemu runner needs an assembled root")
                })?;
                Ok(Selection::Qemu(root))
            }
            Self::Bwrap => match root {
                Some(_root) if host != target => Err(Error::new(
                    codes::RUNNER_002,
                    "bubblewrap cannot run a foreign target; use --runner qemu",
                )),
                Some(root) => Ok(Selection::Bwrap(Some(root))),
                None => Ok(Selection::Bwrap(None)),
            },
            Self::Podman => match image {
                Some(image) => Ok(Selection::Podman(image)),
                None => Err(Error::new(
                    codes::RUNNER_001,
                    "the podman runner needs a builder image",
                )),
            },
            Self::Auto => match root {
                Some(root) if host == target => Ok(Selection::Bwrap(Some(root))),
                Some(root) => Ok(Selection::Qemu(root)),
                None => match image {
                    Some(image) => Ok(Selection::Podman(image)),
                    None => Ok(Selection::Bwrap(None)),
                },
            },
        }
    }

    /// Turn the command-line runner choice and the available roots into one
    /// runner. Every caller must use this, so `auto` cannot drift between
    /// source assembly, rebuild and installed tests.
    pub fn resolve<'a>(
        self,
        host: Arch,
        target: Arch,
        root: Option<&'a Path>,
        image: Option<&'a str>,
    ) -> Result<Box<dyn Runner + 'a>> {
        Ok(match self.choose(host, target, root, image)? {
            Selection::Bwrap(root) => Box::new(BwrapBackend { root }),
            Selection::Qemu(root) => Box::new(QemuBackend { root }),
            Selection::Podman(image) => Box::new(PodmanBackend { image }),
        })
    }
}

pub(crate) fn unsupported(runner: &str, host: Arch, target: Arch) -> Error {
    Error::new(
        codes::RUNNER_002,
        format!(
            "runner '{runner}' cannot execute {target} steps on a {host} host; \
             use a runner that supports this architecture"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::{RunnerKind, Selection};
    use rpmx_base::arch::Arch;
    use std::path::Path;

    fn selected<'a>(
        kind: RunnerKind,
        root: Option<&'a Path>,
        image: Option<&'a str>,
    ) -> Result<Selection<'a>, rpmx_base::error::Error> {
        let host = Arch::X86_64;
        let target = if kind == RunnerKind::Auto {
            Arch::X86_64
        } else {
            host
        };
        kind.choose(host, target, root, image)
    }

    #[test]
    fn native_auto_binds_the_assembled_root() {
        let root = Path::new("/root");
        let selection = selected(RunnerKind::Auto, Some(root), None).expect("native auto");
        assert!(matches!(selection, Selection::Bwrap(Some(path)) if path == root));
    }

    #[test]
    fn explicit_bwrap_with_a_root_binds_it() {
        let root = Path::new("/root");
        let selection = selected(RunnerKind::Bwrap, Some(root), None).expect("native bwrap");
        assert!(matches!(selection, Selection::Bwrap(Some(path)) if path == root));
    }

    #[test]
    fn explicit_bwrap_without_a_root_uses_the_host_root() {
        let selection = selected(RunnerKind::Bwrap, None, None).expect("host bwrap");
        assert!(matches!(selection, Selection::Bwrap(None)));
    }

    #[test]
    fn auto_without_a_root_or_image_uses_the_host_root() {
        let selection = selected(RunnerKind::Auto, None, None).expect("host auto");
        assert!(matches!(selection, Selection::Bwrap(None)));
    }

    #[test]
    fn auto_with_an_image_uses_the_container_backend() {
        let selection = selected(RunnerKind::Auto, None, Some("image")).expect("container auto");
        assert!(matches!(selection, Selection::Podman("image")));
    }

    #[test]
    fn foreign_auto_binds_the_root_inside_qemu() {
        let root = Path::new("/root");
        let selection = RunnerKind::Auto
            .choose(Arch::X86_64, Arch::Aarch64, Some(root), None)
            .expect("foreign auto");
        assert!(matches!(selection, Selection::Qemu(path) if path == root));
    }

    #[test]
    fn explicit_qemu_binds_the_root() {
        let root = Path::new("/root");
        let selection = selected(RunnerKind::Qemu, Some(root), None).expect("explicit qemu");
        assert!(matches!(selection, Selection::Qemu(path) if path == root));
    }

    #[test]
    fn explicit_qemu_without_a_root_is_an_error() {
        let error = selected(RunnerKind::Qemu, None, None).expect_err("qemu needs a root");
        assert_eq!(error.code, "RPMX-ROOT-016");
    }

    #[test]
    fn bwrap_cannot_cross_build() {
        let error = RunnerKind::Bwrap
            .choose(Arch::X86_64, Arch::Aarch64, Some(Path::new("/root")), None)
            .expect_err("bwrap cannot cross");
        assert_eq!(error.code, "RPMX-RUNNER-002");
    }
}
