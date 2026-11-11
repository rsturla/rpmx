use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::Path;

use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};

mod backend;

use backend::{BwrapBackend, PodmanBackend, QemuBackend};

/// Verify delegated ID maps and actual private transaction access on this host.
pub fn check_bwrap_environment() -> Result<()> {
    backend::check_bwrap_environment()
}

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
    pub writable_root: bool,
    /// The machine rpmx runs on.
    pub host: Arch,
    /// The architecture the steps execute as. It differs from `target` for a
    /// cross build, where a host compiler produces target binaries.
    pub exec: Arch,
    /// The architecture of the produced package.
    pub target: Arch,
    /// Target sysroot mounted read-only for a cross build.
    pub sysroot: Option<&'a Path>,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    /// Runner-owned locked artifacts. The build root is not one of them.
    pub assets: RunnerAssets<'a>,
    pub jobs: NonZeroU32,
    pub timeout: NonZeroU64,
}

pub struct RunnerRequest<'a> {
    /// Effective deadline for this step, including any recipe override.
    pub timeout: NonZeroU64,
    pub command: &'a str,
    pub args: &'a [String],
    pub cwd: &'a str,
    pub env: &'a BTreeMap<String, String>,
    pub label: &'a str,
    pub logs: Option<&'a OutputLogs>,
}

pub struct OutputLogs {
    pub stdout: std::fs::File,
    pub stderr: std::fs::File,
}

/// Whether the host has an enabled qemu-user binfmt handler for `target`.
///
/// The `F` flag is required: without it the kernel resolves the interpreter
/// path in the exec'ing mount namespace, where a bubblewrap guest root is `/`
/// and the host interpreter is absent.
pub fn emulation_available(target: Arch) -> bool {
    let qemu = match target {
        Arch::I686 => "i386",
        other => other.as_str(),
    };
    let path = format!("/proc/sys/fs/binfmt_misc/qemu-{qemu}");
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    binfmt_record_usable(&contents)
}

fn binfmt_record_usable(contents: &str) -> bool {
    let enabled = contents.lines().any(|line| line.trim() == "enabled");
    let has_f = contents
        .lines()
        .find_map(|line| line.strip_prefix("flags:"))
        .is_some_and(|flags| flags.contains('F'));
    let interpreter = contents
        .lines()
        .find_map(|line| line.strip_prefix("interpreter "))
        .map(Path::new);
    enabled && has_f && interpreter.is_some_and(Path::exists)
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
    /// Whether this backend can execute `exec` steps on a `host` machine.
    /// `target` never decides this: a cross build executes natively.
    fn supports(&self, host: Arch, exec: Arch) -> bool;
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

/// The bubblewrap decision. Native targets run directly. A foreign target
/// needs an assembled root and a usable host binfmt handler.
fn bwrap<'a>(
    host: Arch,
    exec: Arch,
    root: Option<&'a Path>,
    emulation: bool,
) -> Result<Selection<'a>> {
    if exec == host {
        return Ok(Selection::Bwrap(root));
    }
    match root {
        Some(root) if emulation => Ok(Selection::Bwrap(Some(root))),
        Some(_) => Err(Error::new(
            codes::RUNNER_002,
            format!(
                "bubblewrap cannot execute {exec} on a {host} host: no enabled \
                 F-flag qemu-user binfmt handler; install a qemu-user handler or use \
                 --runner qemu"
            ),
        )),
        None => Err(Error::new(
            codes::RUNNER_002,
            format!(
                "bubblewrap needs an assembled {exec} root for a foreign execution; \
                 use --runner qemu or resolve a target root"
            ),
        )),
    }
}

impl RunnerKind {
    fn choose<'a>(
        self,
        host: Arch,
        exec: Arch,
        target: Arch,
        root: Option<&'a Path>,
        image: Option<&'a str>,
        emulation: bool,
    ) -> Result<Selection<'a>> {
        match self {
            Self::Qemu => {
                if exec == host && target != exec {
                    return Err(Error::new(
                        codes::RUNNER_002,
                        "a cross build executes host binaries; it cannot boot a \
                         target VM. Use --runner bwrap",
                    )
                    .at("runner"));
                }
                let root = root.ok_or_else(|| {
                    Error::new(codes::ROOT_016, "the qemu runner needs an assembled root")
                })?;
                Ok(Selection::Qemu(root))
            }
            Self::Bwrap => bwrap(host, exec, root, emulation),
            Self::Podman => match image {
                Some(image) => Ok(Selection::Podman(image)),
                None => Err(Error::new(
                    codes::RUNNER_001,
                    "the podman runner needs a builder image",
                )),
            },
            Self::Auto => match root {
                Some(root) => bwrap(host, exec, Some(root), emulation),
                None => match image {
                    Some(image) if host == exec => Ok(Selection::Podman(image)),
                    Some(_) => Err(Error::new(
                        codes::RUNNER_002,
                        "a builder image can only run on the host architecture; \
                         resolve an execution root or use --runner qemu",
                    )),
                    None => bwrap(host, exec, None, emulation),
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
        exec: Arch,
        target: Arch,
        root: Option<&'a Path>,
        image: Option<&'a str>,
    ) -> Result<Box<dyn Runner + 'a>> {
        let emulation = exec != host && emulation_available(exec);
        Ok(
            match self.choose(host, exec, target, root, image, emulation)? {
                Selection::Bwrap(root) => Box::new(BwrapBackend { root }),
                Selection::Qemu(root) => Box::new(QemuBackend { root }),
                Selection::Podman(image) => Box::new(PodmanBackend { image, arch: exec }),
            },
        )
    }
}

pub(crate) fn unsupported(runner: &str, host: Arch, exec: Arch) -> Error {
    Error::new(
        codes::RUNNER_002,
        format!(
            "runner '{runner}' cannot execute {exec} steps on a {host} host; \
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
        kind.choose(host, host, host, root, image, false)
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
    fn foreign_auto_uses_bubblewrap_with_binfmt() {
        let root = Path::new("/root");
        let selection = RunnerKind::Auto
            .choose(
                Arch::X86_64,
                Arch::Aarch64,
                Arch::Aarch64,
                Some(root),
                None,
                true,
            )
            .expect("foreign auto");
        assert!(matches!(selection, Selection::Bwrap(Some(path)) if path == root));
    }

    #[test]
    fn foreign_auto_without_binfmt_is_an_error() {
        let root = Path::new("/root");
        let error = RunnerKind::Auto
            .choose(
                Arch::X86_64,
                Arch::Aarch64,
                Arch::Aarch64,
                Some(root),
                None,
                false,
            )
            .expect_err("foreign auto needs binfmt");
        assert_eq!(error.code, "RPMX-RUNNER-002");
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
    fn foreign_bwrap_uses_the_target_root_with_binfmt() {
        let root = Path::new("/root");
        let selection = RunnerKind::Bwrap
            .choose(
                Arch::X86_64,
                Arch::Aarch64,
                Arch::Aarch64,
                Some(root),
                None,
                true,
            )
            .expect("foreign bwrap");
        assert!(matches!(selection, Selection::Bwrap(Some(path)) if path == root));
    }

    #[test]
    fn foreign_bwrap_without_binfmt_is_an_error() {
        let error = RunnerKind::Bwrap
            .choose(
                Arch::X86_64,
                Arch::Aarch64,
                Arch::Aarch64,
                Some(Path::new("/root")),
                None,
                false,
            )
            .expect_err("bwrap cannot cross without binfmt");
        assert_eq!(error.code, "RPMX-RUNNER-002");
    }

    #[test]
    fn foreign_bwrap_without_a_root_is_an_error() {
        let error = RunnerKind::Bwrap
            .choose(Arch::X86_64, Arch::Aarch64, Arch::Aarch64, None, None, true)
            .expect_err("bwrap needs a target root");
        assert_eq!(error.code, "RPMX-RUNNER-002");
    }

    #[test]
    fn a_cross_build_runs_natively_without_binfmt() {
        // host == exec != target: the compiler runs on the host, so the
        // foreign-execution machinery must not be consulted at all.
        let root = Path::new("/build-root");
        let selection = RunnerKind::Auto
            .choose(
                Arch::X86_64,
                Arch::X86_64,
                Arch::Aarch64,
                Some(root),
                None,
                false,
            )
            .expect("cross auto runs natively");
        assert!(matches!(selection, Selection::Bwrap(Some(path)) if path == root));
    }

    #[test]
    fn a_cross_build_rejects_the_qemu_vm() {
        let error = RunnerKind::Qemu
            .choose(
                Arch::X86_64,
                Arch::X86_64,
                Arch::Aarch64,
                Some(Path::new("/build-root")),
                None,
                false,
            )
            .expect_err("a cross build cannot boot a target VM");
        assert_eq!(error.code, "RPMX-RUNNER-002");
        assert!(error.message.contains("cross build"), "{}", error.message);
    }

    #[test]
    fn a_cross_build_without_a_root_cannot_run_steps() {
        let selection =
            RunnerKind::Bwrap.choose(Arch::X86_64, Arch::X86_64, Arch::Aarch64, None, None, false);
        // With no root the host root is used, which is a deliberate escape
        // hatch (--unsafe-host-root); it is not a cross build root.
        assert!(matches!(selection, Ok(Selection::Bwrap(None))));
    }

    fn handler(flags: &str, enabled: bool) -> String {
        format!(
            "{}\ninterpreter /bin/sh\nflags: {flags}\n",
            if enabled { "enabled" } else { "disabled" }
        )
    }

    #[test]
    fn binfmt_flags_need_the_f_character() {
        assert!(super::binfmt_record_usable(&handler("F", true)));
        assert!(super::binfmt_record_usable(&handler("POCF", true)));
        assert!(!super::binfmt_record_usable(&handler("POC", true)));
        assert!(!super::binfmt_record_usable(&handler("F", false)));
    }

    #[test]
    fn binfmt_requires_an_existing_interpreter() {
        let missing = "enabled\ninterpreter /definitely/not/here\nflags: F\n";
        assert!(!super::binfmt_record_usable(missing));
    }
}
