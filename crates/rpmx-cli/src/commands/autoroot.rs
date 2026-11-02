//! Resolve, assemble and cache a build root without being asked to.
//!
//! A recipe may name a profile or describe its root inline. The requirement
//! set is the profile's packages plus every dependency the plan declares,
//! including the `needs` of every resolved step. The result is cached by a
//! digest of that requirement, so a second build does no resolution work.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rpmx_core::appliance;
use rpmx_core::cache_lock::CacheLock;
use rpmx_core::error::codes;
use rpmx_core::hash::sha512_hex;
use rpmx_core::model::{Recipe, RootRepository, RootSpec};
use rpmx_core::plan::Plan;
use rpmx_core::profile::{
    load_profile, Builder, Packages, Profile, Repository, ResolveOptions, DOCUMENT, VERSION,
};
use rpmx_core::root::{
    assemble, load_lock, remove_tree, resolve_profile, write_lock, AssembleOptions, RootLock,
    RootSelection,
};
use rpmx_core::runner::RunnerKind;
use rpmx_core::tree::digest_root;
use rpmx_core::Arch;
use rpmx_core::Error;

use crate::commands::fetch::default_cache;

static APPLIANCE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A root that was resolved or found in the cache.
#[derive(Debug)]
pub struct AutomaticRoot {
    pub lock: PathBuf,
    pub root: PathBuf,
    pub document: RootLock,
    pub profile: String,
    pub packages: usize,
    pub cached: bool,
    /// The target sysroot, when the profile cross-compiles.
    pub sysroot: Option<Box<AutomaticRoot>>,
    /// Toolchain commands for a cross build.
    pub cross: Option<rpmx_core::guest::CrossToolchain>,
}

/// The merged root requirement for a recipe: profile, name and digest.
#[derive(Debug)]
pub struct Requirement {
    /// The root the steps run in: the build root for a cross profile.
    pub profile: Profile,
    pub name: String,
    pub digest: String,
    /// The target sysroot, when the profile cross-compiles.
    pub sysroot: Option<SysrootRequirement>,
    /// Toolchain commands for a cross build.
    pub cross: Option<rpmx_core::guest::CrossToolchain>,
}

/// The target half of a cross build: a second closure with its own identity.
#[derive(Debug)]
pub struct SysrootRequirement {
    pub profile: Profile,
    pub name: String,
    pub digest: String,
}

/// A build root and its optional runner appliance, both assembled.
#[derive(Debug)]
pub struct AssembledRoots {
    pub root: PathBuf,
    pub appliance: Option<PathBuf>,
}

/// A cached root lock for a requirement digest, if this host has one.
pub fn cached_lock_for_requirement(digest: &str) -> Option<RootLock> {
    let cas = default_cache().ok()?;
    let roots = cas.parent().unwrap_or(&cas).join("roots");
    let key = digest.strip_prefix("sha512:").unwrap_or(digest);
    load_lock(&roots.join(key).join("root.lock.yaml")).ok()
}

/// The distribution tag declared by a recipe's named root profile.
pub fn profile_dist_tag(recipe: &Recipe, recipe_path: &Path) -> Result<Option<String>, Error> {
    let Some(spec) = &recipe.build.root else {
        return Ok(None);
    };
    let Some(name) = &spec.profile else {
        return Ok(None);
    };
    let recipe_dir = recipe_path.parent().unwrap_or_else(|| Path::new("."));
    let path = discover_profile(name, recipe_dir)?;
    load_profile(&path)?.dist_tag()
}

/// Build the merged root requirement without touching the network or cache.
///
/// Returns `None` when the recipe names no profile and the caller gave none.
pub fn requirement(
    recipe: &Recipe,
    plan: &Plan,
    recipe_path: &Path,
    profile: Option<&Path>,
    arch: &str,
) -> Result<Option<Requirement>, Error> {
    let spec = recipe.build.root.as_ref();
    let selected_profile = profile.map(Path::to_path_buf);
    if spec.is_none() && selected_profile.is_none() {
        return Ok(None);
    }

    let recipe_dir = recipe_path.parent().unwrap_or_else(|| Path::new("."));
    let mut profile = match (spec, selected_profile.as_deref()) {
        (Some(spec), _) if spec.profile.is_some() => {
            if has_inline_fields(spec) {
                return Err(Error::new(
                    codes::ROOT_006,
                    "build.root must name a profile or describe one inline, not both",
                )
                .at("build.root"));
            }
            let path = discover_profile(spec.profile.as_deref().unwrap_or_default(), recipe_dir)?;
            let profile = load_profile(&path)?;
            reject_recipe_profile_credentials(&profile, &path)?;
            profile
        }
        (Some(spec), _) => inline_profile(spec, arch)?,
        (None, Some(path)) => load_profile(path)?,
        (None, None) => unreachable!("guarded above"),
    };

    let required = required_packages(plan);
    let crossing = profile.cross.is_some();
    if crossing {
        // The build root takes the tools; the target keeps its libraries.
        for package in &required.target {
            push_unique(&mut profile.packages.install, package);
        }
        if let Some(cross) = profile.cross.as_mut() {
            for package in &required.build {
                push_unique(&mut cross.packages, package);
            }
            cross.packages.sort();
        }
    } else {
        for package in required.build.iter().chain(required.target.iter()) {
            push_unique(&mut profile.packages.install, package);
        }
    }
    // Inline roots never pass through `load_profile`; validate the assembled
    // profile before anything derives an architecture or a root path from it.
    profile.validate()?;
    // `archs` is an allowed set. A root is resolved for exactly one target.
    let profile = profile.for_target(arch)?;
    let cross = if crossing {
        let declared = profile.cross().expect("crossing implies a cross block");
        let mut toolchain = rpmx_core::guest::CrossToolchain::new(profile.triple()?);
        toolchain.cflags = declared.cflags.clone();
        toolchain.cxxflags = declared.cxxflags.clone();
        toolchain.ldflags = declared.ldflags.clone();
        Some(toolchain)
    } else {
        None
    };

    let (primary, sysroot) = if crossing {
        let sysroot = profile.sysroot_profile()?;
        (profile.build_root_profile()?, Some(sysroot))
    } else {
        (profile, None)
    };

    let fallback = format!("{}-root", recipe.package.name);
    let name = primary.name(&fallback);
    let digest = digest_profile(&primary)?;
    let sysroot = match sysroot {
        Some(sysroot) => Some(SysrootRequirement {
            name: sysroot.name(&fallback),
            digest: digest_profile(&sysroot)?,
            profile: sysroot,
        }),
        None => None,
    };
    // The build-root requirement is runner-independent. The QEMU appliance is
    // a runner asset, resolved and cached separately from this identity.
    Ok(Some(Requirement {
        profile: primary,
        name,
        digest,
        sysroot,
        cross,
    }))
}

fn digest_profile(profile: &Profile) -> Result<String, Error> {
    let bytes = serde_json::to_vec(profile)
        .map_err(|error| Error::new(codes::ROOT_006, error.to_string()))?;
    Ok(sha512_hex(&bytes))
}

fn push_unique(packages: &mut Vec<String>, package: &String) {
    if !packages.contains(package) {
        packages.push(package.clone());
    }
}

/// Resolve the automatic root for a recipe, if it declares one.
///
/// Returns `None` when the caller supplied explicit root flags or asked for
/// the host root; the existing explicit-root path handles those.
#[allow(clippy::too_many_arguments)]
pub fn ensure(
    recipe: &Recipe,
    plan: &Plan,
    recipe_path: &Path,
    profile: Option<&Path>,
    cache: Option<&Path>,
    offline: bool,
    refresh: bool,
    arch: &str,
) -> Result<Option<AutomaticRoot>, Error> {
    let Some(requirement) = requirement(recipe, plan, recipe_path, profile, arch)? else {
        return Ok(None);
    };
    let mut root = ensure_root(
        &requirement.profile,
        &requirement.name,
        &requirement.digest,
        cache,
        offline,
        refresh,
        arch,
        Assembly::Install,
    )?;
    if let Some(sysroot) = &requirement.sysroot {
        let mut assembled = ensure_root(
            &sysroot.profile,
            &sysroot.name,
            &sysroot.digest,
            cache,
            offline,
            refresh,
            arch,
            Assembly::Extract,
        )?;
        // The build root stays read-only at run time, so the sysroot mount
        // point has to exist before the sandbox starts.
        let mount = root.root.join("sysroot");
        std::fs::create_dir_all(&mount).map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot create {}: {error}", mount.display()),
            )
        })?;
        {
            let _ = &mut assembled;
        }
        root.sysroot = Some(Box::new(assembled));
    }
    root.cross = requirement.cross;
    Ok(Some(root))
}

/// How a resolved closure becomes a directory.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Assembly {
    /// Run the package scripts and record a package database (a build root).
    Install,
    /// Unpack the payload only (a cross sysroot).
    Extract,
}

#[allow(clippy::too_many_arguments)]
fn ensure_root(
    profile: &Profile,
    name: &str,
    digest: &str,
    cache: Option<&Path>,
    offline: bool,
    refresh: bool,
    arch: &str,
    assembly: Assembly,
) -> Result<AutomaticRoot, Error> {
    let profile = profile.clone();
    let name = name.to_string();
    let digest = digest.to_string();
    let cas = match cache {
        Some(path) => path.to_path_buf(),
        None => default_cache()?,
    };
    let roots = cas.parent().unwrap_or(&cas).join("roots");
    let key = digest.strip_prefix("sha512:").unwrap_or(&digest);
    let directory = roots.join(key);
    let lock_path = directory.join("root.lock.yaml");
    let root_path = directory.join("root");

    if !refresh {
        match load_cached(&lock_path, &root_path, arch) {
            Ok(Some(mut root)) => {
                root.sysroot = None;
                return Ok(root);
            }
            Ok(None) => {}
            Err(error) if offline => {
                return Err(Error::new(
                    codes::ROOT_016,
                    format!(
                        "cached root {} does not verify ({error}); run without --offline to rebuild it",
                        directory.display()
                    ),
                ));
            }
            Err(_) => {}
        }
    }

    if offline {
        return Err(Error::new(
            codes::ROOT_016,
            format!(
                "root for '{}' is not cached at {}; run without --offline to resolve it",
                name,
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
    // Serialize publishers per requirement. Without this, two cold starts
    // delete each other's half-built root.
    let _publishing = CacheLock::acquire(&roots, key)?;
    if !refresh {
        if let Ok(Some(mut root)) = load_cached(&lock_path, &root_path, arch) {
            root.sysroot = None;
            return Ok(root);
        }
    }
    if directory.exists() {
        remove_tree(&directory)?;
    }
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot create {}: {error}", directory.display()),
        )
    })?;
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
    let mut document = resolve_profile(&profile, &name, work.path(), &cas)?;
    // The final root is a sibling of the scratch directory: assembly refuses
    // a root inside its own work tree.
    let assembled = match assembly {
        Assembly::Install => assemble(
            &document,
            AssembleOptions {
                cache: &cas,
                work: work.path(),
                root: &root_path,
            },
        )?,
        Assembly::Extract => rpmx_core::root::assemble_sysroot(
            &document,
            AssembleOptions {
                cache: &cas,
                work: work.path(),
                root: &root_path,
            },
        )?,
    };
    document.root_digest = Some(digest_root(&assembled)?);
    // The lock is written last: a concurrent reader sees either a complete
    // root and lock, or no lock and rebuilds.
    write_lock(&lock_path, &document)?;
    let packages = count_packages(&document);
    Ok(AutomaticRoot {
        lock: lock_path,
        root: root_path,
        document,
        profile: name,
        packages,
        cached: false,
        sysroot: None,
        cross: None,
    })
}

/// Assemble a root described by an SRPM-embedded lock, reusing the cache.
///
/// This is what makes `rpmx rebuild foo.src.rpm` self-sufficient without
/// extracting `root.lock.json` by hand.
pub fn ensure_from_lock(
    lock: &RootLock,
    cache: Option<&Path>,
    offline: bool,
    arch: &str,
    needs_appliance: bool,
    override_root: Option<&Path>,
    allow_resolve: bool,
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
    let digest = lock.require_digest()?.to_string();
    let key = digest.strip_prefix("sha512:").unwrap_or(&digest);
    let directory = roots.join(format!("lock-{key}"));
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
            root: root.root,
            appliance,
        });
    }
    if let Some(root) = find_published_root(&roots, &digest) {
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
    let _publishing = CacheLock::acquire(&roots, &format!("lock-{key}"))?;
    if let Ok(Some(root)) = load_cached(&lock_path, &root_path, arch) {
        let appliance = appliance_path(needs_appliance)?;
        return Ok(AssembledRoots {
            root: root.root,
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
    let assembled = assemble(
        lock,
        AssembleOptions {
            cache: &cas,
            work: work.path(),
            root: &root_path,
        },
    )?;
    let actual = digest_root(&assembled)?;
    if actual != digest {
        return Err(Error::new(
            codes::ROOT_017,
            format!("assembled root digest {actual} does not match the SRPM lock's {digest}"),
        ));
    }
    write_lock(&lock_path, lock)?;
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
            return std::fs::canonicalize(root_path).ok();
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
    let digest = lock.require_digest()?.to_string();
    let key = digest.strip_prefix("sha512:").unwrap_or(&digest);
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

fn read_appliance_lock(path: &Path) -> Result<rpmx_core::root::ApplianceLock, Error> {
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

fn write_appliance_lock(path: &Path, lock: &rpmx_core::root::ApplianceLock) -> Result<(), Error> {
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
) -> Result<Option<AutomaticRoot>, Error> {
    if !lock_path.is_file() || !root_path.is_dir() {
        return Ok(None);
    }
    let document = load_lock(lock_path)?;
    // A cache hit must still describe the requested target. The digest walk is
    // expensive, but the target is a cheap identity check.
    document.require_target(arch)?;
    // Publishing writes the lock last, so a root newer than its lock may have
    // been changed after publication. Re-hash only then: a full walk of a
    // distribution root costs minutes on every build.
    let lock_time = std::fs::metadata(lock_path)
        .and_then(|metadata| metadata.modified())
        .ok();
    let root_time = std::fs::metadata(root_path)
        .and_then(|metadata| metadata.modified())
        .ok();
    let changed = match (lock_time, root_time) {
        (Some(lock), Some(root)) => root > lock,
        _ => true,
    };
    if changed {
        RootSelection::verify(&document, root_path, arch)?;
    }
    let path = std::fs::canonicalize(root_path).map_err(|error| {
        Error::new(
            codes::ROOT_017,
            format!("cannot resolve {}: {error}", root_path.display()),
        )
    })?;
    Ok(Some(AutomaticRoot {
        lock: lock_path.to_path_buf(),
        root: path,
        packages: count_packages(&document),
        profile: document.profile.clone(),
        document,
        cached: true,
        sysroot: None,
        cross: None,
    }))
}

/// Report a resolved root on stderr and, with `--verbose`, every package.
///
/// `source` says where the root came from (`assembled`, `cache hit`, `lock`),
/// so a silent cache hit is distinguishable from a first build.
pub fn report_root(lock: &RootLock, profile: &str, source: &str) {
    let count = count_packages(lock);
    rpmx_core::output::phase(
        "root",
        format!(
            "{profile} · {} · {} · {source}",
            lock.archs.join(","),
            rpmx_core::output::count(count, "package", "packages")
        ),
    );
    if let Some(builder) = &lock.builder {
        rpmx_core::output::phase("root", format!("builder {}", builder.image));
    }
    if !lock.repositories.is_empty() {
        rpmx_core::output::phase(
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
    if rpmx_core::output::verbose() {
        for (package_arch, entries) in &lock.packages {
            for package in entries {
                rpmx_core::output::phase(
                    "root",
                    format!("{}-{}.{package_arch}", package.name, package.evr),
                );
            }
        }
    }
}

fn has_inline_fields(spec: &RootSpec) -> bool {
    spec.releasever.is_some()
        || !spec.archs.is_empty()
        || !spec.repositories.is_empty()
        || !spec.packages.is_empty()
        || spec.builder.is_some()
}

fn inline_profile(spec: &RootSpec, arch: &str) -> Result<Profile, Error> {
    let releasever = spec.releasever.clone().ok_or_else(|| {
        Error::new(
            codes::ROOT_006,
            "an inline build.root needs releasever, repositories and packages",
        )
        .at("build.root.releasever")
    })?;
    if spec.repositories.is_empty() {
        return Err(Error::new(
            codes::ROOT_006,
            "an inline build.root needs at least one repository",
        )
        .at("build.root.repositories"));
    }
    let profile = Profile {
        document: DOCUMENT.to_string(),
        version: VERSION,
        name: None,
        releasever,
        dist_tag: None,
        archs: if spec.archs.is_empty() {
            vec![arch.to_string()]
        } else {
            spec.archs.clone()
        },
        builder: Builder {
            image: spec.builder.as_ref().map(|builder| builder.image.clone()),
        },
        repositories: spec.repositories.iter().map(repository).collect(),
        packages: Packages {
            install: spec.packages.clone(),
            reinstall: Vec::new(),
        },
        // An inline root describes one closure; cross needs a profile because
        // it names a second, host-architecture package set.
        cross: None,
        modules: Default::default(),
        options: ResolveOptions::default(),
        paths: Default::default(),
        debuginfo: false,
        sbom: true,
    };
    profile.for_target(arch)
}

fn repository(source: &RootRepository) -> Repository {
    Repository {
        id: source.id.clone(),
        baseurl: source.baseurl.clone(),
        metalink: source.metalink.clone(),
        mirrorlist: source.mirrorlist.clone(),
        gpgkey: source.gpgkey.clone(),
        repo_gpgcheck: source.repo_gpgcheck,
        auth: None,
    }
}

/// Find a profile by path, then by convention.
fn discover_profile(name: &str, recipe_dir: &Path) -> Result<PathBuf, Error> {
    if name.is_empty() {
        return Err(Error::new(codes::ROOT_006, "build.root.profile is empty"));
    }
    // A relative path is relative to the recipe, not the caller's cwd.
    let relative = recipe_dir.join(name);
    if relative.is_file() {
        return Ok(relative);
    }
    let explicit = Path::new(name);
    if explicit.is_file() {
        return Ok(explicit.to_path_buf());
    }
    let mut candidates = vec![
        recipe_dir.join("profiles").join(format!("{name}.yaml")),
        recipe_dir
            .join("profiles")
            .join(format!("{name}.profile.yaml")),
    ];
    if let Some(config) = config_dir() {
        candidates.push(config.join("profiles").join(format!("{name}.yaml")));
    }
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    Err(Error::new(
        codes::ROOT_006,
        format!(
            "profile '{name}' not found; looked at {}",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ))
}

fn config_dir() -> Option<PathBuf> {
    if let Ok(base) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(base).join("rpmx"));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config").join("rpmx"))
}

/// A recipe chooses which profile file it loads. That file may contain
/// repository credentials, so only a profile the user controls may carry them.
fn reject_recipe_profile_credentials(profile: &Profile, path: &Path) -> Result<(), Error> {
    if !profile.repositories.iter().any(|repo| repo.auth.is_some()) {
        return Ok(());
    }
    let trusted = config_dir()
        .map(|config| path_is_under(path, &config.join("profiles")))
        .unwrap_or(false);
    if trusted {
        return Ok(());
    }
    Err(Error::new(
        codes::ROOT_019,
        format!(
            "profile {} was selected by the recipe and carries repository credentials; \
             pass --profile explicitly or place it under {}/profiles",
            path.display(),
            config_dir()
                .map(|config| config.display().to_string())
                .unwrap_or_else(|| "$XDG_CONFIG_HOME/rpmx".to_string())
        ),
    ))
}

/// Compare resolved paths: a lexical `starts_with` would accept `profiles/../..`.
fn path_is_under(path: &Path, root: &Path) -> bool {
    let Ok(path) = std::fs::canonicalize(path) else {
        return false;
    };
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    path.starts_with(root)
}

/// Packages the plan needs, split by the root that owns them.
struct RequiredPackages {
    /// Build-root tools: the compiler, generators, `make`.
    build: BTreeSet<String>,
    /// Target headers and libraries, plus everything a test root needs.
    target: BTreeSet<String>,
}

fn required_packages(plan: &Plan) -> RequiredPackages {
    let mut build = BTreeSet::new();
    let mut target = BTreeSet::new();
    if let Some(source) = &plan.source_assembly {
        for step in &source.steps {
            split_needs(step, false, &mut build, &mut target);
        }
    }
    for cell in &plan.cells {
        build.extend(cell.tools.iter().cloned());
        target.extend(cell.host.iter().cloned());
        target.extend(cell.target.iter().cloned());
        target.extend(cell.test_tools.iter().cloned());
        for step in &cell.pipeline {
            split_needs(step, false, &mut build, &mut target);
        }
        for test in &cell.tests {
            for step in &test.pipeline {
                // Tests run in a target test root.
                split_needs(step, true, &mut build, &mut target);
            }
        }
    }
    RequiredPackages { build, target }
}

fn split_needs(
    step: &rpmx_core::plan::PlannedStep,
    target_only: bool,
    build: &mut BTreeSet<String>,
    target: &mut BTreeSet<String>,
) {
    if step.needs_tools.is_empty() && step.needs_host.is_empty() {
        // A step that only carries the union still has to be installed
        // somewhere. Built-in steps always fill both lists.
        if target_only {
            target.extend(step.needs.iter().cloned());
        } else {
            build.extend(step.needs.iter().cloned());
            target.extend(step.needs.iter().cloned());
        }
        return;
    }
    if target_only {
        target.extend(step.needs_tools.iter().cloned());
    } else {
        build.extend(step.needs_tools.iter().cloned());
    }
    target.extend(step.needs_host.iter().cloned());
}

fn count_packages(lock: &RootLock) -> usize {
    lock.packages.values().map(Vec::len).sum()
}

#[cfg(test)]
mod tests {
    use super::{has_inline_fields, required_packages};
    use rpmx_core::model::Recipe;
    use rpmx_core::root::{
        closure_digest, write_lock, ApplianceLock, LockedBuilder, LockedPackage, APPLIANCE_SCHEMA,
    };
    use std::collections::BTreeMap;

    fn recipe(text: &str) -> Recipe {
        rpmx_core::parse_recipe(text).expect("recipe")
    }

    fn appliance_lock() -> ApplianceLock {
        let archs = vec!["x86_64".to_string()];
        let mut packages = BTreeMap::new();
        packages.insert(
            "x86_64".to_string(),
            vec![LockedPackage {
                name: "kernel-core".to_string(),
                repo_id: "fedora".to_string(),
                evr: "1-1.fc44".to_string(),
                checksum: "sha512:aa".to_string(),
                size: 1,
                srpm: None,
                module: None,
                location: None,
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
                image: "registry.fedoraproject.org/fedora:44".to_string(),
                image_digest: "sha256:builder".to_string(),
            },
            repositories,
            packages,
            closure_digest: closure,
        }
    }

    #[test]
    fn a_profile_name_and_inline_fields_are_mutually_exclusive() {
        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    profile: base
    releasever: "44"
outputs:
  - name: p
    metapackage: true
"#,
        );
        let spec = recipe.build.root.expect("root");
        assert!(has_inline_fields(&spec));
    }

    fn plan(recipe: &Recipe) -> rpmx_core::Plan {
        rpmx_core::plan_with_options(
            recipe,
            rpmx_core::PlanOptions {
                arch: "x86_64",
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan")
    }

    #[test]
    fn examples_name_their_own_build_root() {
        let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let mut checked = 0;
        for entry in std::fs::read_dir(examples).expect("examples directory") {
            let path = entry.expect("entry").path().join("package.yaml");
            let Ok(bundle) = rpmx_core::load_recipe_bundle(&path) else {
                // curl and libfoo are design artifacts for fields that do not
                // exist yet; they cannot build and are not covered here.
                continue;
            };
            let plan = plan(&bundle.recipe);
            let root = super::requirement(&bundle.recipe, &plan, &bundle.root, None, "x86_64")
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert!(
                root.is_some(),
                "{} declares no build.root; examples must build with `rpmx build FILE`",
                path.display()
            );
            checked += 1;
        }
        assert!(checked >= 10, "only checked {checked} examples");
    }

    #[test]
    fn an_inline_root_derives_the_requested_architecture() {
        let directory = tempfile::tempdir().expect("tempdir");
        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    releasever: "44"
    archs: [x86_64, aarch64]
    repositories:
      - id: fedora
        baseurl: https://example.invalid/fedora
    packages: [bash]
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let path = directory.path().join("package.yaml");
        let x86 = super::requirement(&recipe, &plan, &path, None, "x86_64")
            .expect("requirement")
            .expect("root");
        let arm = super::requirement(&recipe, &plan, &path, None, "aarch64")
            .expect("requirement")
            .expect("root");
        assert_eq!(x86.profile.archs, vec!["x86_64".to_string()]);
        assert_eq!(arm.profile.archs, vec!["aarch64".to_string()]);
        assert_ne!(x86.digest, arm.digest);
    }

    #[test]
    fn a_cross_profile_resolves_a_build_root_and_a_sysroot() {
        let directory = tempfile::tempdir().expect("tempdir");
        let native = rpmx_core::Arch::current().expect("host").as_str();
        let target = if native == "x86_64" {
            "aarch64"
        } else {
            "x86_64"
        };
        let profile_path = directory.path().join("cross.yaml");
        std::fs::write(
            &profile_path,
            format!(
                r#"
document: rpmx-profile
version: 1
name: cross
releasever: "44"
archs: [{target}]
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
cross:
  packages: [toolchain-cc]
"#
            ),
        )
        .expect("profile");

        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  dependencies:
    tools: [make]
    host: [zlib-devel]
  pipeline:
    - uses: strip
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let path = directory.path().join("package.yaml");
        let requirement = super::requirement(&recipe, &plan, &path, Some(&profile_path), target)
            .expect("requirement")
            .expect("root");

        // The root the steps run in is native and holds the toolchain.
        assert_eq!(requirement.profile.archs, vec![native.to_string()]);
        assert!(requirement
            .profile
            .packages
            .install
            .contains(&"toolchain-cc".to_string()));
        assert!(requirement
            .profile
            .packages
            .install
            .contains(&"make".to_string()));
        assert!(
            !requirement
                .profile
                .packages
                .install
                .contains(&"zlib-devel".to_string()),
            "a target library cannot live in the host root"
        );

        // The sysroot is the target closure with its libraries.
        let sysroot = requirement.sysroot.expect("sysroot");
        assert_eq!(sysroot.profile.archs, vec![target.to_string()]);
        assert!(sysroot
            .profile
            .packages
            .install
            .contains(&"zlib-devel".to_string()));
        assert!(sysroot
            .profile
            .packages
            .install
            .contains(&"bash".to_string()));
        assert_ne!(
            requirement.digest, sysroot.digest,
            "the two roots need distinct cache identities"
        );
    }

    #[test]
    fn a_native_cache_entry_is_reusable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let cas = directory.path().join("cas");
        std::fs::create_dir_all(&cas).expect("cas");
        let root = directory.path().join("root");
        std::fs::create_dir_all(&root).expect("root");
        let lock = appliance_lock().as_root_lock();
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
        let lock = appliance_lock().as_root_lock();
        let lock_path = directory.path().join("root.lock.yaml");
        write_lock(&lock_path, &lock).expect("lock");

        assert!(
            super::load_cached(&lock_path, &root, "x86_64")
                .expect("load")
                .is_some(),
            "the positive control must hit"
        );
        let error = super::load_cached(&lock_path, &root, "aarch64")
            .expect_err("a cache entry for another architecture must not hit");
        assert_eq!(error.code, rpmx_core::error::codes::ROOT_016);
    }

    #[test]
    fn an_untrusted_lock_cannot_resolve_an_appliance() {
        let directory = tempfile::tempdir().expect("tempdir");
        let cas = directory.path().join("cas");
        std::fs::create_dir_all(&cas).expect("cas");
        let lock = appliance_lock().as_root_lock();
        let error = super::ensure_appliance_for_lock(&lock, directory.path(), &cas, false, false)
            .expect_err("an SRPM lock must not resolve an appliance");
        assert_eq!(error.code, rpmx_core::error::codes::ROOT_020);
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
            rpmx_core::runner::RunnerKind::Qemu,
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
            rpmx_core::runner::RunnerKind::Bwrap,
            "x86_64",
            Some(directory.path()),
            true,
        )
        .expect("native");
        assert!(none.is_none());
    }

    #[test]
    fn inline_roots_are_validated_before_any_resolution() {
        let directory = tempfile::tempdir().expect("tempdir");
        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    releasever: "44"
    archs: [typo]
    repositories:
      - id: repo
        baseurl: https://example.invalid/fedora
    packages: [bash]
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let error = super::ensure(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            Some(directory.path()),
            true,
            false,
            "x86_64",
        )
        .expect_err("invalid arch");
        // `Profile::validate` reports an unknown architecture as ROOT-009.
        assert_eq!(error.code, rpmx_core::error::codes::ROOT_009);
    }

    #[test]
    fn naming_a_profile_and_describing_one_is_rejected() {
        let directory = tempfile::tempdir().expect("tempdir");
        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    profile: base
    releasever: "44"
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let error = super::ensure(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            Some(directory.path()),
            true,
            false,
            "x86_64",
        )
        .expect_err("conflicting root");
        assert_eq!(error.code, rpmx_core::error::codes::ROOT_006);
        assert_eq!(error.path.as_deref(), Some("build.root"));
    }

    #[test]
    fn profile_trust_rejects_traversal_and_symlinks() {
        let directory = tempfile::tempdir().expect("tempdir");
        let trusted = directory.path().join("config/profiles");
        std::fs::create_dir_all(&trusted).expect("profiles");
        let inside = trusted.join("review.yaml");
        std::fs::write(&inside, b"profile").expect("inside");
        assert!(super::path_is_under(&inside, &trusted));

        let outside = directory.path().join("outside.yaml");
        std::fs::write(&outside, b"profile").expect("outside");
        let traversal = trusted.join("../../outside.yaml");
        assert!(!super::path_is_under(&traversal, &trusted));

        #[cfg(unix)]
        {
            let link = trusted.join("linked.yaml");
            std::os::unix::fs::symlink(&outside, &link).expect("symlink");
            assert!(!super::path_is_under(&link, &trusted));
        }
    }

    #[test]
    fn a_recipe_selected_profile_cannot_carry_credentials() {
        let directory = tempfile::tempdir().expect("tempdir");
        let profiles = directory.path().join("profiles");
        std::fs::create_dir_all(&profiles).expect("profiles");
        std::fs::write(
            profiles.join("review.yaml"),
            r#"
document: rpmx-profile
version: 1
name: review
releasever: "44"
archs: [x86_64]
repositories:
  - id: private
    baseurl: https://example.invalid/private
    auth:
      username: builder
      password: env:RPMX_TEST_RECIPE_PASSWORD
packages:
  install: [bash]
"#,
        )
        .expect("profile");
        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    profile: profiles/review.yaml
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let error = super::requirement(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            "x86_64",
        )
        .expect_err("credentials");
        assert_eq!(error.code, rpmx_core::error::codes::ROOT_019);
        assert!(
            error.message.contains("pass --profile"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_profile_path_is_relative_to_the_recipe() {
        let directory = tempfile::tempdir().expect("tempdir");
        let profiles = directory.path().join("profiles");
        std::fs::create_dir_all(&profiles).expect("profiles");
        std::fs::write(
            profiles.join("review.yaml"),
            r#"
document: rpmx-profile
version: 1
name: review
releasever: "44"
archs: [x86_64]
repositories:
  - id: fedora
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
"#,
        )
        .expect("profile");
        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    profile: profiles/review.yaml
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let error = super::ensure(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            Some(directory.path()),
            true,
            false,
            "x86_64",
        )
        .expect_err("cold offline cache");
        assert_eq!(error.code, rpmx_core::error::codes::ROOT_016);
        assert!(
            error.message.contains("not cached"),
            "profile was not discovered: {}",
            error.message
        );
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

    #[test]
    fn plan_dependencies_and_step_needs_join_the_root() {
        let recipe = recipe(
            r#"
schema: 1
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  dependencies:
    tools: [gcc]
    host: [zlib-devel]
    tests: [pytest]
  pipeline:
    - uses: strip
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = rpmx_core::plan_with_options(
            &recipe,
            rpmx_core::PlanOptions {
                arch: "x86_64",
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan");
        let packages = required_packages(&plan);
        // Tools belong to the build root; libraries and test tools to the
        // target sysroot.
        assert!(packages.build.contains("gcc"));
        assert!(!packages.target.contains("gcc"));
        assert!(packages.target.contains("zlib-devel"));
        assert!(packages.target.contains("pytest"));
        // A built-in step contributes its tools to the build root.
        assert!(packages.build.contains("binutils"));
        assert!(packages.build.contains("findutils"));
    }
}
