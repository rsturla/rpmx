use std::path::Path;
use std::process::Command;

use crate::cas;
use crate::error::{codes, Error, Result};
use crate::profile::normalize_arch;
use crate::root::container::{image_digest_for_arch, podman_run_assembly};
use crate::root::lock::{PackageKey, RootLock};

pub struct AssembleOptions<'a> {
    pub cache: &'a Path,
    pub work: &'a Path,
    pub root: &'a Path,
}

/// Install a locked closure into an empty root, offline, and verify that the
/// installed package set matches the lock.
///
/// The root is left in `options.work/root`; the caller computes its digest and
/// records it in the lock.
pub fn assemble(lock: &RootLock, options: AssembleOptions<'_>) -> Result<std::path::PathBuf> {
    let lock_arch = lock
        .archs
        .first()
        .ok_or_else(|| Error::new(codes::ROOT_015, "lock declares no architecture"))?;
    let builder = lock.builder.as_ref().ok_or_else(|| {
        Error::new(
            codes::ROOT_015,
            "this lock has no builder image, so it cannot be assembled",
        )
    })?;
    let actual_digest = image_digest_for_arch(&builder.image, lock_arch)?;
    if actual_digest != builder.image_digest {
        return Err(Error::new(
            codes::ROOT_002,
            format!(
                "builder image '{}' has drifted: lock records {}, local image is {}",
                builder.image, builder.image_digest, actual_digest
            ),
        ));
    }
    let arch = lock
        .archs
        .first()
        .ok_or_else(|| Error::new(codes::ROOT_015, "lock declares no architecture"))?;
    let arch = normalize_arch(arch)?;
    let target = crate::arch::Arch::parse(arch)?;
    let host = crate::arch::Arch::current()?;
    if host != target && !crate::sandbox::emulation_available(target) {
        return Err(Error::new(
            codes::RUNNER_002,
            format!(
                "cannot assemble a {target} root on a {host} host; \
                 no qemu-user binfmt handler is available"
            ),
        ));
    }

    prepare_assembly(options.work, options.root)?;
    let rpms = options.work.join("rpms");

    let mut files = Vec::new();
    for (package_arch, entries) in &lock.packages {
        for package in entries {
            let filename = format!(
                "{}-{}.{package_arch}.rpm",
                package.name,
                package.evr.replace(':', "_")
            );
            let bytes = cas::load(options.cache, &package.checksum)?;
            std::fs::write(rpms.join(&filename), &bytes).map_err(|error| {
                Error::new(
                    codes::ROOT_012,
                    format!("cannot stage {}: {error}", filename),
                )
            })?;
            files.push(format!("/work/rpms/{filename}"));
        }
    }
    if files.is_empty() {
        return Err(Error::new(
            codes::ROOT_015,
            "the lock contains no packages to install",
        ));
    }
    std::fs::create_dir_all(options.root).map_err(|error| {
        Error::new(
            codes::ROOT_012,
            format!("cannot create {}: {error}", options.root.display()),
        )
    })?;
    // `setup` can be installed before `filesystem`; its sysusers trigger
    // creates /lib64 as a real directory, which then makes filesystem's
    // usrmerge symlink fail. Seed the layout before the transaction.
    create_usrmerge_layout(options.root)?;

    let mut args = vec![
        "dnf".to_string(),
        "-y".to_string(),
        "--installroot=/root".to_string(),
        "--config=/etc/dnf/dnf.conf".to_string(),
        "--setopt=reposdir=/work/empty-repos".to_string(),
        "--setopt=cachedir=/work/dnf-cache".to_string(),
        "--setopt=persistdir=/work/dnf-persist".to_string(),
        format!("--releasever={}", lock.releasever),
        format!("--forcearch={arch}"),
        format!(
            "--setopt=install_weak_deps={}",
            u8::from(lock.options.weak_deps)
        ),
    ];
    let mut tsflags = vec!["nocaps"];
    if !lock.options.docs {
        tsflags.insert(0, "nodocs");
    }
    args.push(format!("--setopt=tsflags={}", tsflags.join(",")));
    args.push("install".to_string());
    args.extend(files);

    let output = podman_run_assembly(&builder.image, options.work, options.root, target, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        // The root was empty before this run; remove the partial transaction so
        // the same command can be retried.
        let _ = remove_tree(options.root);
        return Err(Error::new(
            codes::ROOT_012,
            format!("offline install failed:\n{tail}"),
        ));
    }

    verify_installed(lock, options.root, &builder.image, target)?;
    Ok(options.root.to_path_buf())
}

fn prepare_assembly(work: &Path, root: &Path) -> Result<()> {
    if root.starts_with(work) {
        return Err(Error::new(
            codes::ROOT_012,
            format!(
                "the root {} must not be inside the scratch directory {}",
                root.display(),
                work.display()
            ),
        ));
    }
    let staging = work.join("rpms");
    if staging.exists() {
        remove_tree(&staging)?;
    }
    for directory in ["rpms", "empty-repos", "dnf-cache", "dnf-persist"] {
        std::fs::create_dir_all(work.join(directory)).map_err(|error| {
            Error::new(
                codes::ROOT_012,
                format!("cannot create assembly directory {directory}: {error}"),
            )
        })?;
    }
    if root.exists() {
        let empty = std::fs::read_dir(root)
            .map_err(|error| {
                Error::new(
                    codes::ROOT_012,
                    format!("cannot read {}: {error}", root.display()),
                )
            })?
            .next()
            .is_none();
        if !empty {
            return Err(Error::new(
                codes::ROOT_012,
                format!(
                    "root directory {} is not empty; refusing to assemble over it",
                    root.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Seed the usrmerge layout before a fresh rpm transaction. Fedora's
/// `filesystem` package creates these in `%pretrans`, but `setup` may be
/// ordered first and its sysusers trigger otherwise creates `/lib64` as a
/// real directory.
fn create_usrmerge_layout(root: &Path) -> Result<()> {
    for directory in [
        "usr/bin",
        "usr/lib",
        "usr/lib64",
        "usr/local/bin",
        "usr/lib/debug/usr",
    ] {
        std::fs::create_dir_all(root.join(directory)).map_err(|error| {
            Error::new(
                codes::ROOT_012,
                format!("cannot create {}: {error}", root.join(directory).display()),
            )
        })?;
    }
    for (link, target) in [
        ("bin", "usr/bin"),
        ("sbin", "usr/sbin"),
        ("lib", "usr/lib"),
        ("lib64", "usr/lib64"),
        ("usr/sbin", "bin"),
        ("usr/local/sbin", "bin"),
        ("usr/lib/debug/usr/sbin", "bin"),
        ("usr/lib/debug/bin", "usr/bin"),
        ("usr/lib/debug/lib", "usr/lib"),
        ("usr/lib/debug/lib64", "usr/lib64"),
        ("var/run", "../run"),
        ("var/lock", "../run/lock"),
    ] {
        let path = root.join(link);
        if path.exists() || path.symlink_metadata().is_ok() {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::new(
                    codes::ROOT_012,
                    format!("cannot create {}: {error}", parent.display()),
                )
            })?;
        }
        std::os::unix::fs::symlink(target, &path).map_err(|error| {
            Error::new(
                codes::ROOT_012,
                format!("cannot create {}: {error}", path.display()),
            )
        })?;
    }
    Ok(())
}

pub(crate) fn verify_installed(
    lock: &RootLock,
    root: &Path,
    image: &str,
    target: crate::arch::Arch,
) -> Result<()> {
    let mut expected = std::collections::BTreeSet::new();
    for (package_arch, entries) in &lock.packages {
        for package in entries {
            let (epoch, version, release) = evr_parts(&package.evr)?;
            expected.insert(PackageKey::new(
                &package.name,
                &epoch,
                &version,
                &release,
                package_arch,
            ));
        }
    }

    let mut process = Command::new("podman");
    process
        .arg("run")
        .arg("--rm")
        .arg("--pull=missing")
        .arg("--arch")
        .arg(target.to_string());
    process
        .arg("--network=none")
        .arg("--read-only")
        .arg("--cap-drop=all")
        .arg("--security-opt=no-new-privileges")
        .arg("-v")
        .arg(format!("{}:/root:Z", root.display()))
        .arg(image)
        .arg("rpm")
        .arg("--root")
        .arg("/root")
        .arg("-qa")
        .arg("--qf")
        .arg("%{NAME}|%{EPOCH}|%{VERSION}|%{RELEASE}|%{ARCH}\n");
    let output = process.output().map_err(|error| {
        Error::new(
            codes::ROOT_013,
            format!("cannot query the assembled root: {error}"),
        )
    })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::ROOT_013,
            format!(
                "cannot query the assembled root: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    let mut installed = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields = line.split('|').collect::<Vec<_>>();
        if fields.len() != 5 {
            continue;
        }
        installed.insert(PackageKey::new(
            fields[0], fields[1], fields[2], fields[3], fields[4],
        ));
    }

    if installed != expected {
        let missing = expected
            .difference(&installed)
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        let extra = installed
            .difference(&expected)
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        return Err(Error::new(
            codes::ROOT_013,
            format!(
                "installed package set differs from the lock ({} expected, {} installed); missing: {}; extra: {}",
                expected.len(),
                installed.len(),
                missing.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
                extra.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    Ok(())
}

/// Remove a tree that may contain mode-000 files written by rpm.
///
/// Rootless podman maps container root to the calling user, but a directory
/// without write permission still cannot be emptied by its owner. Re-enter the
/// user namespace where the caller is root.
pub fn remove_tree(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(first) => {
            let status = Command::new("podman")
                .arg("unshare")
                .arg("--")
                .arg("rm")
                .arg("-rf")
                .arg(path)
                .status();
            match status {
                Ok(status) if status.success() => Ok(()),
                _ => Err(Error::new(
                    codes::ROOT_012,
                    format!("cannot remove {}: {first}", path.display()),
                )),
            }
        }
    }
}

fn evr_parts(evr: &str) -> Result<(String, String, String)> {
    let (epoch, rest) = match evr.split_once(':') {
        Some((epoch, rest)) => (epoch, rest),
        None => ("0", evr),
    };
    let (version, release) = rest.rsplit_once('-').ok_or_else(|| {
        Error::new(
            codes::ROOT_015,
            format!("package evr '{evr}' has no release separator"),
        )
    })?;
    if epoch.is_empty() || version.is_empty() || release.is_empty() {
        return Err(Error::new(
            codes::ROOT_015,
            format!("package evr '{evr}' is malformed"),
        ));
    }
    Ok((epoch.to_string(), version.to_string(), release.to_string()))
}
