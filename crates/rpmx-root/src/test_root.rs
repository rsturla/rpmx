use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::profile::{Packages, Profile};
use crate::root::{self, RootLock};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};
use rpmx_runner::runner::{emulation_available, RunnerKind};

pub struct TestRootOptions<'a> {
    pub lock: &'a RootLock,
    pub pinned: Option<&'a Path>,
    pub tools: &'a [String],
    pub rpms: &'a [PathBuf],
    pub image: Option<&'a str>,
    pub cache: &'a Path,
    pub work: &'a Path,
    pub runner: RunnerKind,
}

pub struct PreparedTestRoot {
    pub path: PathBuf,
    pub lock: RootLock,
}

/// Build and install a disposable root for installed-stage tests.
pub fn prepare(options: TestRootOptions<'_>) -> Result<PreparedTestRoot> {
    let lock = options.lock;
    let target = Arch::parse(
        lock.archs
            .first()
            .ok_or_else(|| Error::new(codes::BUILD_007, "the test root has no architecture"))?,
    )?;
    let host = Arch::current()?;
    let foreign = host != target || options.runner == RunnerKind::Qemu;
    if host != target && !emulation_available(target) {
        return Err(Error::new(
            codes::RUNNER_002,
            format!(
                "cannot prepare a {target} test root on a {host} host; \
                 no qemu-user binfmt handler is available"
            ),
        ));
    }
    let image = options
        .image
        .or_else(|| lock.builder.as_ref().map(|builder| builder.image.as_str()))
        .ok_or_else(|| {
            Error::new(
                codes::BUILD_007,
                "installed-stage tests need a builder image in the root lock",
            )
        })?;

    let mut install = vec!["bash".to_string(), "coreutils".to_string()];
    if foreign {
        // A QEMU-backed test root must boot itself, so it needs a kernel,
        // initramfs, network, SSH and tar for the workspace transport.
        install.extend(
            [
                "kernel-core",
                "kernel-modules-core",
                "kernel-modules",
                "dracut",
                "kmod",
                "util-linux",
                "systemd",
                "systemd-networkd",
                "openssh-server",
                "tar",
            ]
            .iter()
            .map(|package| package.to_string()),
        );
    }
    for tool in options.tools {
        let name = rpmx_runner::build_env::dependency_name(tool);
        if !name.is_empty() && !install.iter().any(|package| package == name) {
            install.push(name.to_string());
        }
    }

    let profile = Profile {
        document: crate::profile::DOCUMENT.to_string(),
        version: crate::profile::VERSION,
        name: Some("test-root".to_string()),
        releasever: lock.releasever.clone(),
        dist_tag: lock.dist_tag.clone(),
        archs: lock.archs.clone(),
        builder: crate::profile::Builder {
            image: Some(image.to_string()),
        },
        repositories: lock.repositories.clone(),
        packages: Packages {
            install: install.clone(),
            reinstall: Vec::new(),
        },
        modules: Default::default(),
        options: lock.options.clone(),
        debuginfo: false,
        sbom: false,
    };

    let resolve_work = options.work.join("test-resolve");
    let assemble_work = options.work.join("test-assemble");
    let root_path = options.work.join("test-root");
    let test_lock = match options.pinned {
        Some(path) => {
            let pinned = root::load_lock(path)?;
            check_lock(path, &pinned, &install, &lock.archs)?;
            pinned
        }
        None => root::resolve_profile(&profile, "test-root", &resolve_work, options.cache)?,
    };
    root::assemble(
        &test_lock,
        root::AssembleOptions {
            cache: options.cache,
            work: &assemble_work,
            root: &root_path,
        },
    )?;

    let rpms_dir = options
        .rpms
        .first()
        .and_then(|path| path.parent())
        .ok_or_else(|| Error::new(codes::BUILD_007, "no binary RPMs were written"))?;
    // podman treats a relative -v source as a named volume.
    let rpms_dir = std::fs::canonicalize(rpms_dir).map_err(|error| {
        Error::new(
            codes::BUILD_007,
            format!("cannot resolve {}: {error}", rpms_dir.display()),
        )
    })?;
    let output =
        crate::root::container::run_offline_root(crate::root::container::OfflineRootCommand {
            root: &root_path,
            image,
            target,
            mode: crate::root::container::OfflineRootMode::Install { rpms: &rpms_dir },
        })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::BUILD_007,
            format!(
                "cannot install the built RPMs into the test root: {}; \
                 add missing runtime packages to build.dependencies.tests \
                 or pass a pinned --test-root-lock",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    Ok(PreparedTestRoot {
        path: root_path,
        lock: test_lock,
    })
}

/// A pinned test root must contain the packages the recipe asks tests to install,
/// otherwise the lock and the recipe have drifted apart.
fn check_lock(path: &Path, lock: &RootLock, required: &[String], archs: &[String]) -> Result<()> {
    let arch = archs
        .first()
        .ok_or_else(|| Error::new(codes::BUILD_007, "the test root lock has no architectures"))?;
    let installed = lock.packages.get(arch).ok_or_else(|| {
        Error::new(
            codes::BUILD_007,
            format!("the test root lock has no packages for {arch}"),
        )
    })?;
    let names: BTreeSet<&str> = installed
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let missing: Vec<&str> = required
        .iter()
        .map(|dependency| rpmx_runner::build_env::dependency_name(dependency))
        .filter(|name| !name.is_empty() && !names.contains(name))
        .collect();
    if !missing.is_empty() {
        return Err(Error::new(
            codes::BUILD_007,
            format!(
                "test root lock {} is missing test dependencies: {}",
                path.display(),
                missing.join(", ")
            ),
        ));
    }
    Ok(())
}
