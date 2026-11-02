use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::Path;

use crate::runner::{
    unsupported, Runner, RunnerAssets, RunnerConfig, RunnerKind, RunnerRequest, Session,
};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};
use rpmx_base::guest::CrossToolchain;
use rpmx_language::plan::PlannedStep;

/// Deterministic environment for every step. The host environment is never
/// inherited: it can contain credentials and it differs between machines.
const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Variables that define the build contract. A recipe may set toolchain
/// variables, but it may not move the staging tree or rewrite the timestamp.
const RESERVED_ENV: &[&str] = &[
    "SOURCE_DATE_EPOCH",
    "SRC",
    "BUILD",
    "STAGE",
    "PREFIX",
    "LIBDIR",
    "JOBS",
];

/// Everything the sandbox decides before a runner starts: which backend to
/// use, where the build root comes from, and the deterministic policy fields.
#[derive(Clone, Copy)]
pub struct SandboxConfig<'a> {
    pub runner: RunnerKind,
    /// The machine rpmx runs on.
    pub host: Arch,
    /// The architecture the steps execute as. It equals `target` for a native
    /// or emulated build and equals `host` for a cross build.
    pub exec: Arch,
    /// The architecture of the produced package.
    pub target: Arch,
    /// Target sysroot, mounted read-only, when cross-compiling.
    pub sysroot: Option<&'a Path>,
    /// Toolchain commands used when `sysroot` is set.
    pub cross: Option<&'a CrossToolchain>,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    pub root: Option<&'a Path>,
    /// Runner-owned locked artifacts, such as the QEMU appliance.
    pub assets: RunnerAssets<'a>,
    pub image: Option<&'a str>,
    /// Install prefix. The profile supplies it; the runner only exports it.
    pub prefix: &'a str,
    /// Library directory. The profile supplies it; the runner only exports it.
    pub libdir: &'a str,
    pub source_date_epoch: u32,
    pub jobs: NonZeroU32,
    pub timeout: NonZeroU64,
    pub debuginfo: bool,
}

impl<'a> SandboxConfig<'a> {
    fn runner_config(&self) -> RunnerConfig<'a> {
        RunnerConfig {
            host: self.host,
            exec: self.exec,
            target: self.target,
            sysroot: self.sysroot,
            workspace: self.workspace,
            stage: self.stage,
            assets: self.assets,
            jobs: self.jobs,
            timeout: self.timeout,
        }
    }
}

pub struct Sandbox<'a> {
    runner: Box<dyn Runner + 'a>,
    config: SandboxConfig<'a>,
    session: std::cell::RefCell<Option<Box<dyn Session>>>,
}

impl<'a> Sandbox<'a> {
    /// Resolve the configured runner kind and prepare a sandbox.
    pub fn new(config: SandboxConfig<'a>) -> Result<Self> {
        if config.root.is_some() && config.image.is_some() {
            return Err(Error::new(
                codes::ROOT_016,
                "a sandbox cannot combine an assembled root with a builder image",
            ));
        }
        if !config.assets.is_empty() && config.root.is_none() {
            return Err(Error::new(
                codes::ROOT_016,
                "runner assets require an assembled root",
            ));
        }
        prepare_cross_files(&config)?;
        let runner = config.runner.resolve(
            config.host,
            config.exec,
            config.target,
            config.root,
            config.image,
        )?;
        Ok(Self::with_runner(config, runner))
    }

    /// Prepare a sandbox around a caller-provided runner. Compatibility with
    /// the configured host/target pair is checked when the session starts, so
    /// constructing a sandbox that is never used cannot fail on the runner.
    pub fn with_runner(config: SandboxConfig<'a>, runner: Box<dyn Runner + 'a>) -> Self {
        Self {
            runner,
            config,
            session: std::cell::RefCell::new(None),
        }
    }

    pub fn run(&self, step: &PlannedStep) -> Result<()> {
        self.run_at(step, None)
    }

    /// Run one step and report its position in a phase, such as `[2/3] make`.
    pub fn run_numbered(&self, step: &PlannedStep, index: usize, total: usize) -> Result<()> {
        self.run_at(step, Some((index, total)))
    }

    fn run_at(&self, step: &PlannedStep, position: Option<(usize, usize)>) -> Result<()> {
        for key in step.env.keys() {
            if RESERVED_ENV.contains(&key.as_str()) {
                return Err(Error::new(
                    codes::SANDBOX_003,
                    format!(
                        "step '{}' cannot set reserved sandbox variable '{key}'",
                        step.uses
                    ),
                ));
            }
        }

        let command = step.command.as_deref().ok_or_else(|| {
            Error::new(
                codes::BUILD_004,
                format!(
                    "built-in step '{}' has no executable command yet",
                    step.uses
                ),
            )
        })?;
        let args = step
            .args
            .iter()
            .map(|arg| expand(arg, self))
            .collect::<Vec<_>>();
        let mut args = args;
        if rpmx_base::output::debug()
            && command == "sh"
            && args.first().map(String::as_str) == Some("-c")
            && args.len() > 1
        {
            args[1] = format!("set -x\n{}", args[1]);
        }
        let cwd = expand(step.cwd.as_deref().unwrap_or("${source.dir}"), self);

        let mut env = BTreeMap::new();
        for (key, value) in base_env(self) {
            env.insert(key.to_string(), value);
        }
        for (key, value) in &step.env {
            env.insert(key.clone(), expand(value, self));
        }

        let label = if step.label.is_empty() {
            step.uses.clone()
        } else {
            step.label.clone()
        };
        let heading = step_heading(step, position);
        let request = RunnerRequest {
            command,
            args: &args,
            cwd: &cwd,
            env: &env,
            label: &label,
        };

        let mut session = self.session.borrow_mut();
        if session.is_none() {
            if !self.runner.supports(self.config.host, self.config.exec) {
                return Err(unsupported(
                    self.runner.name(),
                    self.config.host,
                    self.config.exec,
                ));
            }
            *session = Some(self.runner.start(&self.config.runner_config())?);
        }
        let mut rendered = String::new();
        rendered.push_str(command);
        for arg in &args {
            rendered.push(' ');
            rendered.push_str(arg);
        }
        // Step headers and outcomes go to stderr so stdout stays a clean
        // artifact list (or a JSON document).
        let stage = rpmx_base::output::current_phase();
        if rpmx_base::output::debug() {
            match stage {
                Some(phase) => rpmx_base::output::phase(phase, format!("run {rendered}")),
                None => rpmx_base::output::phase("run", &rendered),
            }
        }
        let step_line = match stage {
            Some(_) => format!("step {heading}"),
            None => heading.clone(),
        };
        rpmx_base::output::phase(stage.unwrap_or("step"), &step_line);
        let started = std::time::Instant::now();
        let result = session
            .as_mut()
            .expect("session was initialized")
            .run(&request);
        match result {
            Ok(()) => {
                if rpmx_base::output::verbose() || rpmx_base::output::debug() {
                    let step_line = match stage {
                        Some(_) => format!("step {heading}"),
                        None => heading.clone(),
                    };
                    rpmx_base::output::phase(
                        stage.unwrap_or("step"),
                        format!("{step_line} · ok ({:.1}s)", started.elapsed().as_secs_f64()),
                    );
                }
                Ok(())
            }
            Err(mut error) => {
                let help = if rpmx_base::output::keep_workdir() {
                    "the work directory was kept for inspection"
                } else {
                    "re-run with --keep-workdir to inspect the workspace"
                };
                error.message.push_str(&format!(
                    "\n  command: {rendered}\n  cwd: {cwd}\n  workspace: {}\n  stage: {}\n  help: {help}",
                    self.config.workspace.display(),
                    self.config.stage.display()
                ));
                Err(error)
            }
        }
    }

    /// Flush session-local outputs to the host workspace and stage.
    pub fn sync(&self) -> Result<()> {
        if let Some(session) = self.session.borrow_mut().as_mut() {
            session.sync_out()?;
        }
        Ok(())
    }
}

/// The user-facing name of a step, without the phase prefix the planner adds.
///
/// Unnamed `runs:` steps become `runs#1`; the ordinal is already in the
/// position, so show only `runs`.
fn step_name(step: &PlannedStep) -> Option<&str> {
    let raw = if step.label.is_empty() {
        step.uses.as_str()
    } else {
        step.label.as_str()
    };
    let raw = raw
        .strip_prefix("source:")
        .or_else(|| raw.strip_prefix("build:"))
        .or_else(|| raw.strip_prefix("test:"))
        .unwrap_or(raw);
    if raw.is_empty() {
        None
    } else {
        Some(raw.split('#').next().unwrap_or(raw))
    }
}

fn step_heading(step: &PlannedStep, position: Option<(usize, usize)>) -> String {
    match (position, step_name(step)) {
        (Some((index, total)), Some(name)) => format!("[{index}/{total}] {name}"),
        (Some((index, total)), None) => format!("[{index}/{total}]"),
        (None, Some(name)) => name.to_string(),
        (None, None) => String::new(),
    }
}

impl Drop for Sandbox<'_> {
    fn drop(&mut self) {
        if let Some(mut session) = self.session.get_mut().take() {
            let _ = session.stop();
        }
    }
}

/// A readable label for an unnamed inline step.
fn base_env(sandbox: &Sandbox<'_>) -> Vec<(&'static str, String)> {
    environment(&sandbox.config)
}

fn environment(config: &SandboxConfig<'_>) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("PATH", SANDBOX_PATH.to_string()),
        ("HOME", rpmx_base::guest::BUILD.to_string()),
        ("TMPDIR", "/tmp".to_string()),
        ("LANG", "C.UTF-8".to_string()),
        ("LC_ALL", "C.UTF-8".to_string()),
        ("TZ", "UTC".to_string()),
        ("SOURCE_DATE_EPOCH", config.source_date_epoch.to_string()),
        ("SRC", rpmx_base::guest::BUILD.to_string()),
        ("BUILD", rpmx_base::guest::BUILD.to_string()),
        ("STAGE", rpmx_base::guest::STAGE.to_string()),
        ("PREFIX", config.prefix.to_string()),
        ("LIBDIR", config.libdir.to_string()),
        ("JOBS", config.jobs.to_string()),
        ("RPMX_HOST_ARCH", config.host.to_string()),
        ("RPMX_EXEC_ARCH", config.exec.to_string()),
        ("RPMX_TARGET_ARCH", config.target.to_string()),
        (
            "RPMX_EMULATED",
            if config.exec != config.host { "1" } else { "0" }.to_string(),
        ),
        (
            "RPMX_CROSS",
            if config.sysroot.is_some() { "1" } else { "0" }.to_string(),
        ),
        (
            "RPMX_DEBUGINFO",
            if config.debuginfo { "1" } else { "0" }.to_string(),
        ),
    ];
    let mut cflags = String::new();
    let mut cxxflags = String::new();
    if config.debuginfo {
        cflags.push_str("-g");
        cxxflags.push_str("-g");
    }
    if let (Some(toolchain), Some(_)) = (config.cross, config.sysroot) {
        let sysroot = rpmx_base::guest::SYSROOT;
        for (flags, extra) in [
            (&mut cflags, &toolchain.cflags),
            (&mut cxxflags, &toolchain.cxxflags),
        ] {
            if !flags.is_empty() {
                flags.push(' ');
            }
            flags.push_str(&format!("--sysroot={sysroot}"));
            for flag in extra {
                flags.push(' ');
                flags.push_str(flag);
            }
        }
    }
    if !cflags.is_empty() {
        env.push(("CFLAGS", cflags));
    }
    if !cxxflags.is_empty() {
        env.push(("CXXFLAGS", cxxflags));
    }
    if let (Some(toolchain), Some(_sysroot)) = (config.cross, config.sysroot) {
        env.extend(cross_env(config, toolchain));
    }
    env
}

/// CMake and Meson cannot be told to cross-compile with environment variables
/// alone, so the executor materialises their toolchain files in the workspace.
/// A recipe passes `${RPMX_CMAKE_TOOLCHAIN_FILE}` or
/// `${RPMX_MESON_CROSS_FILE}`; it never writes the file itself.
fn prepare_cross_files(config: &SandboxConfig<'_>) -> Result<()> {
    let (Some(toolchain), Some(_sysroot)) = (config.cross, config.sysroot) else {
        return Ok(());
    };
    let directory = config.workspace.join(".rpmx");
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::new(
            codes::SANDBOX_001,
            format!("cannot create {}: {error}", directory.display()),
        )
    })?;
    let arch = config.target;
    let sysroot = rpmx_base::guest::SYSROOT;
    let libdir = config.libdir;
    let endian = if arch.is_big_endian() {
        "big"
    } else {
        "little"
    };
    let cmake = format!(
        "# Generated by rpmx for a {arch} cross build. Do not edit.
set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR {arch})
set(CMAKE_C_COMPILER {cc})
set(CMAKE_CXX_COMPILER {cxx})
set(CMAKE_AR {ar})
set(CMAKE_LINKER {ld})
set(CMAKE_SYSROOT {sysroot})
set(CMAKE_FIND_ROOT_PATH {sysroot})
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
set(CMAKE_EXE_LINKER_FLAGS \"--sysroot={sysroot} -Wl,-rpath-link,{sysroot}{libdir}\")
set(CMAKE_SHARED_LINKER_FLAGS \"--sysroot={sysroot} -Wl,-rpath-link,{sysroot}{libdir}\")
set(PKG_CONFIG_EXECUTABLE {pkg_config})
",
        cc = toolchain.cc,
        cxx = toolchain.cxx,
        ar = toolchain.ar,
        ld = toolchain.ld,
        pkg_config = toolchain.pkg_config,
    );
    std::fs::write(directory.join("toolchain.cmake"), cmake).map_err(|error| {
        Error::new(
            codes::SANDBOX_001,
            format!("cannot write the CMake toolchain file: {error}"),
        )
    })?;
    let meson = format!(
        "# Generated by rpmx for a {arch} cross build. Do not edit.
[binaries]
c = '{cc}'
cpp = '{cxx}'
ar = '{ar}'
strip = '{strip}'
pkg-config = '{pkg_config}'

[host_machine]
system = 'linux'
cpu_family = '{arch}'
cpu = '{arch}'
endian = '{endian}'

[properties]
sys_root = '{sysroot}'
pkg_config_libdir = ['{sysroot}{libdir}/pkgconfig', '{sysroot}{prefix}/share/pkgconfig']
needs_exe_wrapper = true
",
        cc = toolchain.cc,
        cxx = toolchain.cxx,
        ar = toolchain.ar,
        strip = toolchain.strip,
        pkg_config = toolchain.pkg_config,
        prefix = config.prefix,
    );
    std::fs::write(directory.join("meson.cross"), meson).map_err(|error| {
        Error::new(
            codes::SANDBOX_001,
            format!("cannot write the Meson cross file: {error}"),
        )
    })?;
    Ok(())
}

/// Environment that turns native tools into a cross toolchain.
///
/// Everything is expressed in guest paths: the sysroot is mounted at
/// [`rpmx_base::guest::SYSROOT`] and the libraries live under the profile's
/// `PREFIX`/`LIBDIR`, so no distribution layout is assumed.
fn cross_env(
    config: &SandboxConfig<'_>,
    toolchain: &CrossToolchain,
) -> Vec<(&'static str, String)> {
    let sysroot = rpmx_base::guest::SYSROOT;
    let libdir = config.libdir;
    let prefix = config.prefix;
    let pkgconfig = format!("{sysroot}{libdir}/pkgconfig:{sysroot}{prefix}/share/pkgconfig");
    vec![
        ("RPMX_TARGET_TRIPLE", toolchain.triple.clone()),
        ("RPMX_HOST_TRIPLE", config.host.triple().to_string()),
        ("RPMX_SYSROOT", sysroot.to_string()),
        ("GOOS", "linux".to_string()),
        ("GOARCH", config.target.go_arch().to_string()),
        // A distribution package links the system C library on purpose: the
        // pure-Go resolver and user lookup ignore nsswitch.conf and NSS.
        ("CGO_ENABLED", "1".to_string()),
        ("CC", toolchain.cc.clone()),
        ("CXX", toolchain.cxx.clone()),
        ("AR", toolchain.ar.clone()),
        ("LD", toolchain.ld.clone()),
        ("STRIP", toolchain.strip.clone()),
        ("OBJCOPY", toolchain.objcopy.clone()),
        ("PKG_CONFIG", toolchain.pkg_config.clone()),
        ("PKG_CONFIG_SYSROOT_DIR", sysroot.to_string()),
        ("PKG_CONFIG_LIBDIR", pkgconfig),
        ("LDFLAGS", {
            let mut value = format!("--sysroot={sysroot} -Wl,-rpath-link,{sysroot}{libdir}");
            for flag in &toolchain.ldflags {
                value.push(' ');
                value.push_str(flag);
            }
            value
        }),
        (
            "RPMX_CMAKE_TOOLCHAIN_FILE",
            format!("{}/.rpmx/toolchain.cmake", rpmx_base::guest::BUILD),
        ),
        (
            "RPMX_MESON_CROSS_FILE",
            format!("{}/.rpmx/meson.cross", rpmx_base::guest::BUILD),
        ),
    ]
}

fn expand(value: &str, sandbox: &Sandbox<'_>) -> String {
    value
        .replace("${source.dir}", rpmx_base::guest::BUILD)
        .replace("${build.dir}", rpmx_base::guest::BUILD)
        .replace("${stage}", rpmx_base::guest::STAGE)
        .replace("${prefix}", sandbox.config.prefix)
        .replace("${libdir}", sandbox.config.libdir)
        .replace("${jobs}", &sandbox.config.jobs.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Sandbox, SandboxConfig};
    use crate::runner::{RunnerAssets, RunnerKind};
    use rpmx_base::arch::Arch;
    use std::num::{NonZeroU32, NonZeroU64};
    use std::path::Path;

    #[test]
    fn a_cross_environment_uses_guest_paths_and_the_profile_layout() {
        use rpmx_base::guest::CrossToolchain;

        let toolchain = CrossToolchain::new("aarch64-linux-gnu");
        let config = SandboxConfig {
            runner: RunnerKind::Auto,
            host: Arch::X86_64,
            exec: Arch::X86_64,
            target: Arch::Aarch64,
            sysroot: Some(Path::new("/cache/sysroots/arm")),
            cross: Some(&toolchain),
            workspace: Path::new("/tmp"),
            stage: Path::new("/tmp"),
            root: Some(Path::new("/root")),
            assets: RunnerAssets::default(),
            image: None,
            prefix: "/usr",
            libdir: "/usr/lib64",
            source_date_epoch: 1,
            jobs: NonZeroU32::new(1).expect("jobs"),
            timeout: NonZeroU64::new(1).expect("timeout"),
            debuginfo: true,
        };
        let env: std::collections::BTreeMap<_, _> =
            super::environment(&config).into_iter().collect();
        assert_eq!(env["CC"], "aarch64-linux-gnu-gcc");
        assert_eq!(env["CXX"], "aarch64-linux-gnu-g++");
        assert_eq!(env["RPMX_SYSROOT"], "/sysroot");
        assert_eq!(env["RPMX_TARGET_TRIPLE"], "aarch64-linux-gnu");
        assert_eq!(
            env["PKG_CONFIG_LIBDIR"],
            "/sysroot/usr/lib64/pkgconfig:/sysroot/usr/share/pkgconfig"
        );
        assert_eq!(env["PKG_CONFIG_SYSROOT_DIR"], "/sysroot");
        assert!(
            env["CFLAGS"].contains("--sysroot=/sysroot"),
            "{}",
            env["CFLAGS"]
        );
        assert!(env["CFLAGS"].contains("-g"), "{}", env["CFLAGS"]);
        assert!(env["LDFLAGS"].contains("-rpath-link,/sysroot/usr/lib64"));
        assert_eq!(env["RPMX_CROSS"], "1");
        assert_eq!(env["RPMX_EMULATED"], "0");
        assert_eq!(env["RPMX_EXEC_ARCH"], "x86_64");
    }

    #[test]
    fn a_cross_sandbox_writes_the_build_system_toolchain_files() {
        use rpmx_base::guest::CrossToolchain;

        let toolchain = CrossToolchain::new("aarch64-linux-gnu");
        let workspace = tempfile::tempdir().expect("workspace");
        let config = SandboxConfig {
            runner: RunnerKind::Auto,
            host: Arch::X86_64,
            exec: Arch::X86_64,
            target: Arch::Aarch64,
            sysroot: Some(Path::new("/cache/sysroots/arm")),
            cross: Some(&toolchain),
            workspace: workspace.path(),
            stage: Path::new("/tmp"),
            root: Some(Path::new("/root")),
            assets: RunnerAssets::default(),
            image: None,
            prefix: "/usr",
            libdir: "/usr/lib64",
            source_date_epoch: 1,
            jobs: NonZeroU32::new(1).expect("jobs"),
            timeout: NonZeroU64::new(1).expect("timeout"),
            debuginfo: false,
        };
        super::prepare_cross_files(&config).expect("toolchain files");

        let cmake = std::fs::read_to_string(workspace.path().join(".rpmx/toolchain.cmake"))
            .expect("cmake file");
        assert!(
            cmake.contains("set(CMAKE_C_COMPILER aarch64-linux-gnu-gcc)"),
            "{cmake}"
        );
        assert!(cmake.contains("set(CMAKE_SYSROOT /sysroot)"), "{cmake}");
        assert!(
            cmake.contains("CMAKE_CROSSCOMPILING") || cmake.contains("CMAKE_SYSTEM_NAME Linux")
        );

        let meson = std::fs::read_to_string(workspace.path().join(".rpmx/meson.cross"))
            .expect("meson file");
        assert!(meson.contains("cpu_family = 'aarch64'"), "{meson}");
        assert!(meson.contains("endian = 'little'"), "{meson}");
        assert!(meson.contains("needs_exe_wrapper = true"), "{meson}");
        assert!(meson.contains("/sysroot/usr/lib64/pkgconfig"), "{meson}");
    }

    #[test]
    fn a_root_and_an_image_cannot_both_be_configured() {
        let config = SandboxConfig {
            runner: RunnerKind::Auto,
            host: Arch::X86_64,
            exec: Arch::X86_64,
            target: Arch::X86_64,
            sysroot: None,
            cross: None,
            workspace: Path::new("/tmp"),
            stage: Path::new("/tmp"),
            root: Some(Path::new("/root")),
            assets: RunnerAssets::default(),
            image: Some("image"),
            prefix: "/usr",
            libdir: "/usr/lib64",
            source_date_epoch: 1,
            jobs: NonZeroU32::new(1).expect("jobs"),
            timeout: NonZeroU64::new(3600).expect("timeout"),
            debuginfo: false,
        };
        let error = match Sandbox::new(config) {
            Ok(_) => panic!("root and image must be mutually exclusive"),
            Err(error) => error,
        };
        assert_eq!(error.code, "RPMX-ROOT-016");
    }
}
