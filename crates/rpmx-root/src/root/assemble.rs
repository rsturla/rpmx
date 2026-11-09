use std::path::{Path, PathBuf};
use std::process::Command;

use crate::profile::normalize_arch;
use crate::root::container::{image_digest_for_arch, podman_run_assembly};
use crate::root::lock::{safe_staging_component, PackageKey, RootLock};
use rpmx_base::error::{codes, Error, Result};
use rpmx_store::cas;

pub struct AssembleOptions<'a> {
    pub cache: &'a Path,
    pub work: &'a Path,
    pub root: &'a Path,
    /// RPMs installed in the same transaction as the lock, from their own
    /// files rather than the cache. A test root uses it for the packages under
    /// test: they are not in any repository, and another locked package may
    /// require a capability only they provide.
    pub extra_rpms: &'a [PathBuf],
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
    let target = rpmx_base::arch::Arch::parse(arch)?;
    let host = rpmx_base::arch::Arch::current()?;
    if host != target && !rpmx_runner::runner::emulation_available(target) {
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
    let mut staged_closure = Vec::new();
    for (package_arch, entries) in &lock.packages {
        for package in entries {
            if !safe_staging_component(package_arch)
                || !safe_staging_component(&package.name)
                || !safe_staging_component(&package.evr)
            {
                return Err(Error::new(
                    codes::ROOT_012,
                    "the lock contains a package name, EVR, or architecture that cannot be staged safely",
                ));
            }
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
            rpmx_base::output::installing(format!(
                "{}-{}.{package_arch}",
                package.name, package.evr
            ));
            staged_closure.push(rpms.join(&filename));
            files.push(format!("/work/rpms/{filename}"));
        }
    }
    let locked_files = files.clone();
    let mut extra_files = Vec::new();
    let mut staged_extras = Vec::new();
    let mut extra_keys = Vec::new();
    for rpm in options.extra_rpms {
        let metadata = rpmx_payload::metadata::package_metadata(rpm)?;
        let filename = format!(
            "{}-{}.{}.rpm",
            metadata.name,
            metadata.evr().replace(':', "_"),
            metadata.arch
        );
        if !safe_staging_component(&metadata.name)
            || !safe_staging_component(&metadata.evr())
            || !safe_staging_component(&metadata.arch)
        {
            return Err(Error::new(
                codes::ROOT_012,
                format!("cannot stage {}: unsafe package name", rpm.display()),
            ));
        }
        std::fs::copy(rpm, rpms.join(&filename)).map_err(|error| {
            Error::new(
                codes::ROOT_012,
                format!("cannot stage {}: {error}", rpm.display()),
            )
        })?;
        rpmx_base::output::installing(format!(
            "{}-{}.{}",
            metadata.name,
            metadata.evr(),
            metadata.arch
        ));
        staged_extras.push(rpms.join(&filename));
        extra_files.push(format!("/work/rpms/{filename}"));
        extra_keys.push((
            metadata.name.clone(),
            metadata.epoch,
            metadata.version.clone(),
            metadata.release.clone(),
            metadata.arch.clone(),
        ));
    }
    if locked_files.is_empty() {
        return Err(Error::new(
            codes::ROOT_015,
            "the lock contains no packages to install",
        ));
    }
    prepare_root_dir(options.root)?;

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
    let base = args;
    let locked = locked_files;

    let run = |args: &[String]| -> Result<std::process::Output> {
        podman_run_assembly(&builder.image, options.work, options.root, target, args)
    };
    let combined = |erase: bool, files: &[String]| -> Vec<String> {
        let mut args = base.clone();
        if erase {
            // dnf5 only takes the option after the command.
            let command = args
                .iter()
                .position(|argument| argument == "install")
                .expect("the install command is part of the arguments");
            args.insert(command + 1, "--allowerasing".to_string());
        }
        args.extend(files.iter().cloned());
        args
    };
    let all_files = {
        let mut files = locked.clone();
        files.extend(extra_files.iter().cloned());
        files
    };

    let install_extras = |nodeps: bool| -> Result<std::process::Output> {
        crate::root::container::run_offline_root(crate::root::container::OfflineRootCommand {
            root: options.root,
            image: &builder.image,
            target,
            nodeps,
            mode: crate::root::container::OfflineRootMode::Install {
                rpms: options.extra_rpms,
            },
        })
    };

    // The lock is the distribution's own closure, so it installs first. The
    // packages under test come next, replacing files where the distribution
    // splits the same library differently. A package under test may also
    // replace what the lock holds outright; rpm reports that as a conflict and
    // the retry installs the packages under test without dependency checking.
    let mut replaced = false;
    let mut output = run(&combined(false, &locked))?;
    if output.status.success() && !extra_files.is_empty() {
        output = install_extras(false)?;
        if !output.status.success()
            && is_replacement_conflict(&String::from_utf8_lossy(&output.stderr))
        {
            replaced = true;
            output = install_extras(true)?;
        }
    }
    if !output.status.success() {
        // The closure alone did not install, because one of its packages needs
        // a capability only the build provides. Install everything in one
        // transaction, erasing what the packages under test replace.
        rpmx_base::output::warning(
            "test-root",
            "the packages under test fill a gap in the test root or replace \
             what it already has; installing both in one transaction",
        );
        reset_root(options.root)?;
        output = run(&combined(true, &all_files))?;
        replaced = true;
    }
    if !output.status.success() && !extra_files.is_empty() {
        // A file that both the distribution's package and the package under
        // test ship cannot be resolved by erasing: the distribution's package
        // stays required. Install everything with rpm instead, which replaces
        // the file the way an upgrade does, and skip dependency checking,
        // because the packages under test are the only providers of some of
        // what the closure requires.
        reset_root(options.root)?;
        let mut staged = staged_closure.clone();
        staged.extend(staged_extras.iter().cloned());
        output =
            crate::root::container::run_offline_root(crate::root::container::OfflineRootCommand {
                root: options.root,
                image: &builder.image,
                target,
                nodeps: true,
                mode: crate::root::container::OfflineRootMode::Install { rpms: &staged },
            })?;
        replaced = true;
    }
    if !output.status.success() {
        return Err(offline_failure(&output));
    }

    verify_installed(
        lock,
        options.root,
        &builder.image,
        target,
        &extra_keys,
        replaced,
    )?;
    Ok(options.root.to_path_buf())
}

/// Unpack a locked closure into a link-time sysroot.
///
/// Unlike [`assemble`], nothing in the payload is executed: no scriptlets, no
/// `dnf`, no target binary. A cross sysroot is headers and libraries that the
/// host compiler reads; it is never booted and its package database is not
/// needed. This is what lets a cross build run on a host with no qemu-user
/// handler at all.
pub fn assemble_sysroot(
    lock: &RootLock,
    options: AssembleOptions<'_>,
) -> Result<std::path::PathBuf> {
    prepare_assembly(options.work, options.root)?;
    let rpms = options.work.join("rpms");
    std::fs::create_dir_all(options.root).map_err(|error| {
        Error::new(
            codes::ROOT_012,
            format!("cannot create {}: {error}", options.root.display()),
        )
    })?;

    let mut extracted = 0usize;
    for (package_arch, entries) in &lock.packages {
        for package in entries {
            if !safe_staging_component(package_arch)
                || !safe_staging_component(&package.name)
                || !safe_staging_component(&package.evr)
            {
                return Err(Error::new(
                    codes::ROOT_012,
                    "the lock contains a package name, EVR, or architecture that cannot be staged safely",
                ));
            }
            let filename = format!(
                "{}-{}.{package_arch}.rpm",
                package.name,
                package.evr.replace(':', "_")
            );
            let path = rpms.join(&filename);
            let bytes = cas::load(options.cache, &package.checksum)?;
            std::fs::write(&path, &bytes).map_err(|error| {
                Error::new(
                    codes::ROOT_012,
                    format!("cannot stage {}: {error}", filename),
                )
            })?;
            let payload = rpm::Package::open(&path).map_err(|error| {
                Error::new(
                    codes::ROOT_012,
                    format!("cannot read {}: {error}", filename),
                )
            })?;
            rpmx_payload::extract_payload(&payload, options.root).map_err(|error| {
                Error::new(
                    codes::ROOT_012,
                    format!("cannot unpack {}: {error}", filename),
                )
            })?;
            rpmx_base::output::installing(format!(
                "{}-{}.{package_arch}",
                package.name, package.evr
            ));
            extracted += 1;
        }
    }
    if extracted == 0 {
        return Err(Error::new(
            codes::ROOT_015,
            "the lock contains no packages to extract",
        ));
    }
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
        // `filesystem` creates these in %pretrans. A transaction that skips
        // scriptlets (a package under test that replaces the distribution's)
        // leaves the runner without the mount points it binds.
        "proc",
        "sys",
        "dev",
        "tmp",
        "run",
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
    target: rpmx_base::arch::Arch,
    extra_keys: &[(String, u32, String, String, String)],
    allow_missing: bool,
) -> Result<()> {
    let mut expected = std::collections::BTreeSet::new();
    for (name, epoch, version, release, arch) in extra_keys {
        expected.insert(PackageKey::new(
            name,
            &epoch.to_string(),
            version,
            release,
            arch,
        ));
    }
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

    let output = super::container::run_offline_root(super::container::OfflineRootCommand {
        root,
        image,
        target,
        nodeps: false,
        mode: super::container::OfflineRootMode::Query,
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

    if allow_missing && expected.is_subset(&installed) {
        return Ok(());
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
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
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

/// Whether an install failure is a replacement conflict rather than a missing
/// requirement.
///
/// A package under test may deliberately replace what the test root already
/// has, for example a package that provides the same library as the
/// distribution's default implementation. A genuine missing requirement must
/// stay an error, so a failure that mentions one never takes the retry.
fn is_replacement_conflict(stderr: &str) -> bool {
    let conflict = stderr.contains("conflicts with")
        || stderr.contains("conflicts between attempted installs");
    let missing = stderr.contains("is needed by") || stderr.contains("Requires:");
    conflict && !missing
}

/// Report a failed offline install with the tail of what dnf or rpm said.
fn offline_failure(output: &std::process::Output) -> Error {
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
    Error::new(codes::ROOT_012, format!("offline install failed:\n{tail}"))
}

/// Start the transaction from an empty root again.
///
/// The staged RPMs stay in the work directory: only the root is discarded.
fn reset_root(root: &Path) -> Result<()> {
    remove_tree(root)?;
    prepare_root_dir(root)
}

/// Create the empty root and the directories a first transaction needs.
fn prepare_root_dir(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root).map_err(|error| {
        Error::new(
            codes::ROOT_012,
            format!("cannot create {}: {error}", root.display()),
        )
    })?;
    // `setup` can be installed before `filesystem`; its sysusers trigger
    // creates /lib64 as a real directory, which then makes filesystem's
    // usrmerge symlink fail. Seed the layout before the transaction.
    create_usrmerge_layout(root)?;
    // The `filesystem` package makes `/` read-only, so the sysroot mount
    // point has to exist before the transaction runs.
    std::fs::create_dir_all(root.join("sysroot")).map_err(|error| {
        Error::new(
            codes::ROOT_012,
            format!("cannot create the sysroot mount point: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::is_replacement_conflict;

    #[test]
    fn a_replacement_conflict_takes_the_retry_but_a_missing_requirement_does_not() {
        // The zlib example: the test root's gcc pulls zlib-ng-compat, which
        // the package under test replaces.
        assert!(is_replacement_conflict(
            "error: Failed dependencies:\n\
             \tzlib(x86-64) conflicts with (installed) zlib-ng-compat-2.3.3-3.fc44.x86_64"
        ));

        assert!(!is_replacement_conflict(
            "error: Failed dependencies:\n\
             \tlibc.so.6()(64bit) is needed by zlib-1.3.1-1.fc44.x86_64"
        ));

        // Both in one report: the missing dependency is the real problem.
        assert!(!is_replacement_conflict(
            "error: Failed dependencies:\n\
             \tzlib(x86-64) conflicts with (installed) zlib-ng-compat\n\
             \tlibc.so.6()(64bit) is needed by zlib-1.3.1-1.fc44.x86_64"
        ));

        assert!(!is_replacement_conflict(""));
    }
}
