//! Resolve, assemble and cache a build root without being asked to.
//!
//! A recipe may name a profile or describe its root inline. The requirement
//! set is the profile's packages plus every dependency the plan declares,
//! including the `needs` of every resolved step. The result is cached by a
//! digest of that requirement, so a second build does no resolution work.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::domain::appliance;
use crate::domain::cache_lock::CacheLock;
use crate::domain::error::codes;
use crate::domain::profile::Profile;
use crate::domain::root::{
    assemble, load_lock, remove_tree, resolve_profile, write_lock, AssembleOptions, RootLock,
    RootSelection,
};
use crate::domain::runner::RunnerKind;
use crate::domain::tree::digest_root;
use crate::domain::Arch;
use crate::domain::Error;
pub(crate) use rpmx_root::recipe::{declared_arch, requirement};

use crate::support::default_cache;

static APPLIANCE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A build root and its optional runner appliance, both assembled.
#[derive(Debug)]
pub struct AssembledRoots {
    pub root: PathBuf,
    pub appliance: Option<PathBuf>,
}

/// Resolve a requirement independently of its assembled root cache.
pub(crate) fn resolve_root(
    profile: &Profile,
    name: &str,
    digest: &str,
    cache: &Path,
    offline: bool,
) -> Result<RootLock, Error> {
    if let Some(lock) = rpmx_root::recipe::cached_lock_for_requirement(cache, digest) {
        return Ok(lock);
    }
    let base = cache.parent().unwrap_or(cache).join("resolved-roots");
    let key = digest.trim_start_matches("sha512:");
    let path = base.join(format!("{key}.yaml"));
    if path.is_file() {
        let lock = load_lock(&path)?;
        lock.require_target(profile.arch())?;
        return Ok(lock);
    }
    if offline {
        return Err(Error::new(
            codes::ROOT_016,
            format!("root for '{name}' is not cached; prepare it online first"),
        ));
    }
    std::fs::create_dir_all(&base)
        .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
    let _publishing = CacheLock::acquire(&base, key)?;
    if path.is_file() {
        let lock = load_lock(&path)?;
        lock.require_target(profile.arch())?;
        return Ok(lock);
    }
    let work = crate::support::WorkDir::new("resolve-root")?;
    let lock = resolve_profile(profile, name, work.path(), cache)?;
    let temporary = tempfile::NamedTempFile::new_in(&base)
        .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
    write_lock(temporary.path(), &lock)?;
    temporary
        .persist(&path)
        .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
    Ok(lock)
}

/// How a resolved closure becomes a directory.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Assembly {
    Install,
    Extract,
}

/// Create the guest sysroot mount point before a root digest is computed.
///
/// The sandbox mounts the target sysroot at a fixed guest path, and the
/// assembled build root is mounted read-only, so the directory must already
/// exist. Creating it before the digest keeps a cache hit from looking newer
/// than its lock.
fn create_sysroot_mount(root: &Path) -> Result<(), Error> {
    let relative = crate::domain::guest::SYSROOT.trim_start_matches('/');
    let mount = root.join(relative);
    std::fs::create_dir_all(&mount).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot create {}: {error}", mount.display()),
        )
    })
}

/// Assemble a host build root described by an SRPM-embedded lock.
#[allow(clippy::too_many_arguments)]
pub fn ensure_from_lock(
    lock: &RootLock,
    cache: Option<&Path>,
    offline: bool,
    arch: &str,
    needs_appliance: bool,
    override_root: Option<&Path>,
    allow_resolve: bool,
    mount_sysroot: bool,
) -> Result<AssembledRoots, Error> {
    ensure_with(
        lock,
        cache,
        offline,
        arch,
        needs_appliance,
        override_root,
        allow_resolve,
        Assembly::Install,
        mount_sysroot,
    )
}

/// Unpack a target sysroot described by an SRPM-embedded lock.
///
/// The sysroot is a link-time artefact: payloads only, no scriptlets and no
/// emulation, which is what lets a cross rebuild run on any host.
pub fn ensure_sysroot_from_lock(
    lock: &RootLock,
    cache: Option<&Path>,
    offline: bool,
    arch: &str,
) -> Result<PathBuf, Error> {
    Ok(ensure_with(
        lock,
        cache,
        offline,
        arch,
        false,
        None,
        false,
        Assembly::Extract,
        false,
    )?
    .root)
}

#[allow(clippy::too_many_arguments)]
fn ensure_with(
    lock: &RootLock,
    cache: Option<&Path>,
    offline: bool,
    arch: &str,
    needs_appliance: bool,
    override_root: Option<&Path>,
    allow_resolve: bool,
    assembly: Assembly,
    mount_sysroot: bool,
) -> Result<AssembledRoots, Error> {
    let cas = match cache {
        Some(path) => path.to_path_buf(),
        None => default_cache()?,
    };
    if let Some(root) = override_root {
        if !root.is_dir() {
            return Err(Error::new(
                codes::ROOT_020,
                format!("the QEMU appliance {} is not a directory", root.display()),
            ));
        }
    }
    let appliance_override = override_root.map(Path::to_path_buf);
    let roots = cas.parent().unwrap_or(&cas).join("roots");
    lock.validate()?;
    lock.require_target(arch)?;
    let digest = lock
        .root_digest
        .as_ref()
        .map(|_| lock.require_digest().map(str::to_string))
        .transpose()?;
    let identity = lock.assembly_identity();
    let key = identity.strip_prefix("sha512:").unwrap_or(&identity);
    let prefix = if assembly == Assembly::Extract {
        "sysroot"
    } else {
        "lock"
    };
    let directory = roots.join(format!("{prefix}-{key}"));
    let appliance_path = |needs: bool| -> Result<Option<PathBuf>, Error> {
        if !needs {
            return Ok(None);
        }
        if let Some(root) = &appliance_override {
            return Ok(Some(root.clone()));
        }
        ensure_appliance_for_lock(lock, &directory, &cas, offline, allow_resolve)
    };
    let lock_path = directory.join("root.lock.yaml");
    let root_path = directory.join("root");

    if let Ok(Some(root)) = load_cached(&lock_path, &root_path, arch) {
        let appliance = appliance_path(needs_appliance)?;
        return Ok(AssembledRoots {
            root: root.path().to_path_buf(),
            appliance,
        });
    }
    if let Some(root) = digest
        .as_deref()
        .and_then(|digest| find_published_root(&roots, digest))
    {
        let appliance = appliance_path(needs_appliance)?;
        return Ok(AssembledRoots { root, appliance });
    }
    if offline {
        return Err(Error::new(
            codes::ROOT_016,
            format!(
                "the root recorded in this SRPM is not cached at {}; run without --offline to assemble it",
                directory.display()
            ),
        ));
    }

    std::fs::create_dir_all(&roots).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot create {}: {error}", roots.display()),
        )
    })?;
    let _publishing = CacheLock::acquire(&roots, &format!("{prefix}-{key}"))?;
    if let Ok(Some(root)) = load_cached(&lock_path, &root_path, arch) {
        let appliance = appliance_path(needs_appliance)?;
        return Ok(AssembledRoots {
            root: root.path().to_path_buf(),
            appliance,
        });
    }
    // Replacing the build root must not discard a previously resolved
    // appliance sidecar; a later qemu rebuild depends on it.
    let sidecar = directory.join(APPLIANCE_LOCK_FILE);
    let saved_sidecar = std::fs::read(&sidecar).ok();
    if directory.exists() {
        remove_tree(&directory)?;
    }
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot create {}: {error}", directory.display()),
        )
    })?;
    if let Some(bytes) = saved_sidecar {
        std::fs::write(&sidecar, bytes).map_err(|error| {
            Error::new(
                codes::ROOT_020,
                format!("cannot restore {}: {error}", sidecar.display()),
            )
        })?;
    }
    let work = tempfile::Builder::new()
        .prefix("rpmx-root-")
        .tempdir_in(&roots)
        .map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!(
                    "cannot create a work directory in {}: {error}",
                    roots.display()
                ),
            )
        })?;
    let assembled = match assembly {
        Assembly::Install => assemble(
            lock,
            AssembleOptions {
                cache: &cas,
                work: work.path(),
                root: &root_path,
            },
        )?,
        Assembly::Extract => crate::domain::root::assemble_sysroot(
            lock,
            AssembleOptions {
                cache: &cas,
                work: work.path(),
                root: &root_path,
            },
        )?,
    };
    if mount_sysroot {
        create_sysroot_mount(&assembled)?;
    }
    let actual = digest_root(&assembled)?;
    if digest.as_ref().is_some_and(|digest| &actual != digest) {
        return Err(Error::new(
            codes::ROOT_017,
            format!("assembled root digest {actual} does not match the SRPM lock"),
        ));
    }
    let mut receipt = lock.clone();
    receipt.root_digest = Some(actual);
    receipt.rpmdb_digest = Some(rpmx_store::tree::rpmdb_digest(&assembled)?);
    write_lock(&lock_path, &receipt)?;
    let appliance = appliance_path(needs_appliance)?;
    Ok(AssembledRoots {
        root: root_path,
        appliance,
    })
}

/// Find an already-published root with the same digest under a different
/// requirement key. A build's automatic root and an SRPM's embedded lock often
/// describe the same tree.
fn find_published_root(roots: &Path, digest: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(roots).ok()?;
    for entry in entries.flatten() {
        let directory = entry.path();
        let lock_path = directory.join("root.lock.yaml");
        let root_path = directory.join("root");
        if !lock_path.is_file() || !root_path.is_dir() {
            continue;
        }
        let Ok(lock) = load_lock(&lock_path) else {
            continue;
        };
        if lock.root_digest.as_deref() == Some(digest) {
            return RootSelection::verify(&lock, &root_path, lock.archs.first()?.as_str())
                .ok()
                .map(|selection| selection.path().to_path_buf());
        }
    }
    None
}

/// Ensure the appliance described by a loaded lock, when the selected runner
/// needs a VM. Native builds never pay for an appliance they cannot use.
pub fn appliance_for(
    lock: &RootLock,
    cache: Option<&Path>,
    offline: bool,
    runner: RunnerKind,
    arch: &str,
    override_root: Option<&Path>,
    allow_resolve: bool,
) -> Result<Option<PathBuf>, Error> {
    let needs =
        appliance::Policy::Auto.should_resolve(Arch::current()?, Arch::parse(arch)?, runner);
    if !needs {
        return Ok(None);
    }
    if let Some(root) = override_root {
        if !root.is_dir() {
            return Err(Error::new(
                codes::ROOT_020,
                format!("the QEMU appliance {} is not a directory", root.display()),
            ));
        }
        return Ok(Some(root.to_path_buf()));
    }
    let cas = match cache {
        Some(path) => path.to_path_buf(),
        None => default_cache()?,
    };
    let roots = cas.parent().unwrap_or(&cas).join("roots");
    lock.require_digest()?;
    let identity = lock.assembly_identity();
    let key = identity.strip_prefix("sha512:").unwrap_or(&identity);
    let directory = roots.join(format!("lock-{key}"));
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::new(
            codes::ROOT_020,
            format!("cannot create {}: {error}", directory.display()),
        )
    })?;
    let _publishing = CacheLock::acquire(&roots, &format!("lock-{key}"))?;
    ensure_appliance_for_lock(lock, &directory, &cas, offline, allow_resolve)
}

/// Load a lock and ensure its appliance, when the runner needs a VM.
pub fn appliance_for_lock(
    lock_path: Option<&Path>,
    cache: Option<&Path>,
    offline: bool,
    runner: RunnerKind,
    arch: &str,
    override_root: Option<&Path>,
) -> Result<Option<PathBuf>, Error> {
    let Some(path) = lock_path else {
        return Ok(None);
    };
    let lock = load_lock(path)?;
    appliance_for(&lock, cache, offline, runner, arch, override_root, true)
}

/// Assemble the locked appliance into its content-addressed cache.
const APPLIANCE_LOCK_FILE: &str = "appliance.lock.yaml";

fn read_appliance_lock(path: &Path) -> Result<crate::domain::root::ApplianceLock, Error> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::ROOT_020,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    serde_yaml_ng::from_str(&text).map_err(|error| {
        Error::new(
            codes::ROOT_020,
            format!("invalid appliance lock {}: {error}", path.display()),
        )
    })
}

fn write_appliance_lock(
    path: &Path,
    lock: &crate::domain::root::ApplianceLock,
) -> Result<(), Error> {
    let text = serde_yaml_ng::to_string(lock)
        .map_err(|error| Error::new(codes::ROOT_020, error.to_string()))?;
    let nonce = APPLIANCE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("tmp.{}.{}", std::process::id(), nonce));
    std::fs::write(&temporary, text).map_err(|error| {
        Error::new(
            codes::ROOT_020,
            format!("cannot write {}: {error}", temporary.display()),
        )
    })?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        Error::new(
            codes::ROOT_020,
            format!("cannot publish {}: {error}", path.display()),
        )
    })
}

/// Load the appliance sidecar, or resolve it from the lock's distribution
/// inputs when `allow_resolve` is true.
///
/// An SRPM-embedded lock is untrusted: `ensure_from_lock` passes false, so a
/// rebuild never turns attacker-controlled repository URLs into a network
/// fetch and VM content. An explicit root lock supplied by the user may
/// resolve.
fn ensure_appliance_for_lock(
    lock: &RootLock,
    directory: &Path,
    cas: &Path,
    offline: bool,
    allow_resolve: bool,
) -> Result<Option<PathBuf>, Error> {
    let path = directory.join(APPLIANCE_LOCK_FILE);
    let appliance = if path.is_file() {
        read_appliance_lock(&path)?
    } else {
        if !allow_resolve {
            return Err(Error::new(
                codes::ROOT_020,
                format!(
                    "the root lock has no QEMU appliance sidecar at {}; run a qemu build once or pass --runner-asset-root",
                    path.display()
                ),
            ));
        }
        if offline {
            return Err(Error::new(
                codes::ROOT_020,
                format!(
                    "the QEMU appliance is not cached at {}; run without --offline or pass --appliance-root",
                    path.display()
                ),
            ));
        }
        let profile = appliance::profile_from_lock(lock)?;
        let work = tempfile::Builder::new()
            .prefix("rpmx-appliance-")
            .tempdir_in(directory.parent().unwrap_or(directory))
            .map_err(|error| {
                Error::new(
                    codes::ROOT_020,
                    format!("cannot create an appliance work directory: {error}"),
                )
            })?;
        let appliance = appliance::resolve(&profile, &lock.profile, work.path(), cas)?;
        std::fs::create_dir_all(directory).map_err(|error| {
            Error::new(
                codes::ROOT_020,
                format!("cannot create {}: {error}", directory.display()),
            )
        })?;
        write_appliance_lock(&path, &appliance)?;
        appliance
    };
    appliance::ensure_cached(&appliance, cas, offline).map(Some)
}

fn load_cached(
    lock_path: &Path,
    root_path: &Path,
    arch: &str,
) -> Result<Option<RootSelection>, Error> {
    if !lock_path.is_file() || !root_path.is_dir() {
        return Ok(None);
    }
    let document = load_lock(lock_path)?;
    let selection = RootSelection::verify(&document, root_path, arch)?;
    Ok(Some(selection))
}

/// Report a resolved root on stderr and, with `--verbose`, every package.
///
/// `source` says where the root came from (`assembled`, `cache hit`, `lock`),
/// so a silent cache hit is distinguishable from a first build.
pub fn report_root(lock: &RootLock, profile: &str, source: &str) {
    let count = count_packages(lock);
    crate::domain::output::phase(
        "root",
        format!(
            "{profile} · {} · {} · {source}",
            lock.archs.join(","),
            crate::domain::output::count(count, "package", "packages")
        ),
    );
    if let Some(builder) = &lock.builder {
        crate::domain::output::phase(
            "root",
            format!("builder {}", builder.installer.pinned_reference()),
        );
    }
    if !lock.repositories.is_empty() {
        crate::domain::output::phase(
            "root",
            format!(
                "repositories {}",
                lock.repositories
                    .iter()
                    .map(|repository| repository.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    if crate::domain::output::verbose() {
        for (package_arch, entries) in &lock.packages {
            for package in entries {
                crate::domain::output::phase(
                    "root",
                    format!("{}-{}.{package_arch}", package.name, package.evr),
                );
            }
        }
    }
}

fn count_packages(lock: &RootLock) -> usize {
    lock.packages.values().map(Vec::len).sum()
}

#[cfg(test)]
mod tests {
    use crate::domain::root::{
        closure_digest, write_lock, ApplianceLock, LockedBuilder, LockedImage, LockedPackage,
        PackageOrigin, APPLIANCE_SCHEMA,
    };
    use std::collections::BTreeMap;

    fn appliance_lock() -> ApplianceLock {
        let archs = vec!["x86_64".to_string()];
        let mut packages = BTreeMap::new();
        packages.insert(
            "x86_64".to_string(),
            vec![LockedPackage {
                name: "kernel-core".to_string(),
                origin: PackageOrigin::Candidate,
                evr: "1-1.fc44".to_string(),
                checksum: format!("sha512:{}", "a".repeat(128)),
                size: 1,
                srpm: None,
                module: None,
            }],
        );
        let repositories = Vec::new();
        let closure = closure_digest("44", &archs, &repositories, &packages);
        ApplianceLock {
            schema: APPLIANCE_SCHEMA,
            profile: "test-qemu-appliance".to_string(),
            archs,
            releasever: "44".to_string(),
            builder: LockedBuilder {
                resolver: LockedImage::new(
                    "registry.fedoraproject.org/fedora:44".into(),
                    format!("sha256:{}", "b".repeat(64)),
                    crate::domain::arch::native_arch(),
                )
                .expect("resolver"),
                installer: LockedImage::new(
                    "registry.fedoraproject.org/fedora:44".into(),
                    format!("sha256:{}", "c".repeat(64)),
                    "x86_64",
                )
                .expect("installer"),
            },
            repositories,
            packages,
            closure_digest: closure,
        }
    }

    #[test]
    fn the_sysroot_mount_point_is_covered_by_the_root_digest() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path().join("root");
        std::fs::create_dir_all(&root).expect("root");
        let before = crate::domain::tree::digest_root(&root).expect("digest");
        super::create_sysroot_mount(&root).expect("mount point");
        assert!(root.join("sysroot").is_dir());
        let after = crate::domain::tree::digest_root(&root).expect("digest");
        assert_ne!(
            before, after,
            "the mount point must be created before the digest is published"
        );
    }

    #[test]
    fn a_native_cache_entry_is_reusable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let cas = directory.path().join("cas");
        std::fs::create_dir_all(&cas).expect("cas");
        let root = directory.path().join("root");
        std::fs::create_dir_all(&root).expect("root");
        let mut lock = appliance_lock().as_root_lock();
        lock.root_digest = Some(crate::domain::tree::digest_root(&root).expect("digest"));
        lock.rpmdb_digest = Some(rpmx_store::tree::rpmdb_digest(&root).expect("database digest"));
        let lock_path = directory.path().join("root.lock.yaml");
        write_lock(&lock_path, &lock).expect("lock");
        assert!(super::load_cached(&lock_path, &root, "x86_64")
            .expect("load")
            .is_some());
    }

    #[test]
    fn a_cache_hit_for_another_architecture_is_rejected() {
        let directory = tempfile::tempdir().expect("tempdir");
        let cas = directory.path().join("cas");
        std::fs::create_dir_all(&cas).expect("cas");
        let root = directory.path().join("root");
        std::fs::create_dir_all(&root).expect("root");
        let mut lock = appliance_lock().as_root_lock();
        lock.root_digest = Some(crate::domain::tree::digest_root(&root).expect("digest"));
        lock.rpmdb_digest = Some(rpmx_store::tree::rpmdb_digest(&root).expect("database digest"));
        let lock_path = directory.path().join("root.lock.yaml");
        write_lock(&lock_path, &lock).expect("lock");

        assert!(
            super::load_cached(&lock_path, &root, "x86_64")
                .expect("load")
                .is_some(),
            "the positive control must hit"
        );
        let error = super::load_cached(&lock_path, &root, "aarch64")
            .err()
            .expect("a cache entry for another architecture must not hit");
        assert_eq!(error.code, crate::domain::error::codes::ROOT_016);
    }

    #[test]
    fn an_untrusted_lock_cannot_resolve_an_appliance() {
        let directory = tempfile::tempdir().expect("tempdir");
        let cas = directory.path().join("cas");
        std::fs::create_dir_all(&cas).expect("cas");
        let lock = appliance_lock().as_root_lock();
        let error = super::ensure_appliance_for_lock(&lock, directory.path(), &cas, false, false)
            .expect_err("an SRPM lock must not resolve an appliance");
        assert_eq!(error.code, crate::domain::error::codes::ROOT_020);
        assert!(error.message.contains("sidecar"));
    }

    #[test]
    fn an_explicit_runner_asset_root_bypasses_the_cache() {
        let directory = tempfile::tempdir().expect("tempdir");
        let lock = appliance_lock().as_root_lock();
        let path = super::appliance_for(
            &lock,
            None,
            true,
            crate::domain::runner::RunnerKind::Qemu,
            "x86_64",
            Some(directory.path()),
            true,
        )
        .expect("appliance")
        .expect("override");
        assert_eq!(path, directory.path());

        let none = super::appliance_for(
            &lock,
            None,
            true,
            crate::domain::runner::RunnerKind::Bwrap,
            "x86_64",
            Some(directory.path()),
            true,
        )
        .expect("native");
        assert!(none.is_none());
    }

    #[test]
    fn publishing_the_same_requirement_is_serialized() {
        let directory = tempfile::tempdir().expect("tempdir");
        let held = super::CacheLock::acquire(directory.path(), "key").expect("first lock");
        let path = directory.path().to_path_buf();
        let started = std::time::Instant::now();
        let waiter = std::thread::spawn(move || {
            let _second = super::CacheLock::acquire(&path, "key").expect("second lock");
            started.elapsed()
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        let waiting = waiter.is_finished();
        drop(held);
        let elapsed = waiter.join().expect("thread");
        assert!(!waiting, "second publisher did not wait");
        assert!(
            elapsed >= std::time::Duration::from_millis(150),
            "second publisher returned after {elapsed:?}"
        );
    }
}
