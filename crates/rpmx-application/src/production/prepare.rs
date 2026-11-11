use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::domain::error::codes;
use crate::domain::root::{load_lock, RootSelection};
use crate::domain::srpm::load_lock_file;
use crate::domain::steps::validate_name;
use crate::domain::Error;
use rpmx_rpm::extract::extract_srpm;

use crate::support::{load_plan, reconcile_policy, PlanPolicy};

use super::RebuildContext;

pub(super) struct RebuildState {
    pub(super) work: crate::support::WorkDir,
    pub(super) extracted: rpmx_rpm::extract::ExtractedSrpm,
    pub(super) package_lock: crate::domain::srpm::PackageLock,
    pub(super) bundled: crate::domain::components::Inventory,
    pub(super) bundle: crate::domain::RecipeBundle,
    pub(super) recipe: crate::domain::Recipe,
    pub(super) plan: crate::domain::Plan,
    pub(super) cell: crate::domain::plan::PlannedCell,
    pub(super) pipelines: PathBuf,
    pub(super) build: PathBuf,
    pub(super) stage: PathBuf,
    pub(super) active_lock: Option<crate::domain::root::RootLock>,
    pub(super) locked_packages: BTreeSet<String>,
    pub(super) assembled_root: Option<RootSelection>,
    pub(super) assembled_appliance: Option<PathBuf>,
    pub(super) signature_timestamp: Option<u32>,
    /// Where the steps run: the host for a cross SRPM.
    pub(super) exec: crate::domain::Arch,
    /// Assembled target sysroot for a cross SRPM.
    pub(super) sysroot: Option<PathBuf>,
    /// Toolchain recorded in the SRPM's cross block.
    pub(super) cross: Option<crate::domain::guest::CrossToolchain>,
    /// The sysroot lock, for the dependency check and installed tests.
    pub(super) sysroot_lock: Option<crate::domain::root::RootLock>,
    /// Architecture the package is for: the sysroot's for a cross SRPM.
    pub(super) target: String,
    /// Old embedded roots replaced by `rpmx bootstrap rebuild`.
    pub(super) replaced: Option<crate::domain::srpm::ReplacedLock>,
}

pub(super) fn prepare(
    request: &RebuildContext<'_>,
    signing: Option<&rpmx_rpm::sign::SigningKey>,
) -> Result<RebuildState, Error> {
    let work = crate::support::WorkDir::new("rebuild")?;
    let extracted = extract_srpm(request.input.srpm.as_path(), work.path())?;
    // Read the authoritative lock before unpacking the snapshot. The snapshot
    // is user-controlled recipe data and must not be able to replace it.
    let package_lock = load_lock_file(&work.path().join("package.lock.json"))?;
    // A cross SRPM names its target sysroot; load it before planning so the
    // plan is expanded for the architecture the package is for.
    let embedded_cross = package_lock.cross.clone();
    let cross_lock_and_arch = match &embedded_cross {
        Some(recorded) => {
            let path = work.path().join(&recorded.sysroot.path);
            let bytes = std::fs::read(&path).map_err(|error| {
                Error::new(
                    codes::SRPM_006,
                    format!("cannot read the embedded sysroot lock: {error}"),
                )
            })?;
            let digest = crate::domain::srpm::sha512_hex(&bytes);
            if digest != recorded.sysroot.digest {
                return Err(Error::new(
                    codes::SRPM_006,
                    "the embedded sysroot lock does not match the digest recorded in the package lock",
                ));
            }
            let lock = load_lock(&path)?;
            let arch = lock.archs.first().ok_or_else(|| {
                Error::new(codes::ROOT_015, "the sysroot lock declares no architecture")
            })?;
            let arch = crate::domain::profile::normalize_arch(arch)?.to_string();
            Some((lock, arch))
        }
        None => None,
    };
    let target_arch = cross_lock_and_arch
        .as_ref()
        .map(|(_, arch)| arch.clone())
        .unwrap_or_else(|| request.arch.to_string());
    // The SRPM header is what `rpm` thinks the package is; packaging it for a
    // different architecture would silently mislabel it.
    let header_arch = rpmx_rpm::package_arch(request.input.srpm.as_path())?;
    if header_arch != "noarch" && header_arch != target_arch {
        return Err(Error::new(
            codes::SRPM_006,
            format!(
                "the SRPM is for {header_arch}, but the build target is {target_arch}; \
                 rebuild without --arch or pick the matching architecture"
            ),
        ));
    }
    let signature_timestamp = crate::support::signature_timestamp(
        request.config.signing.signing_timestamp,
        extracted.source_date_epoch,
        signing,
    )?;
    let build = work.path().join("build");
    std::fs::create_dir_all(&build).map_err(|error| {
        Error::new(
            codes::BUILD_005,
            format!("cannot create {}: {error}", build.display()),
        )
    })?;
    let snapshot = work.path().join("RPMX-SOURCE.tar.gz");
    if snapshot.exists() {
        crate::domain::source::unpack_snapshot(&snapshot, &build)?;
    }
    let bundled = crate::domain::components::Inventory::read(
        &work.path().join(crate::domain::components::INVENTORY_FILE),
    )?;

    let pipelines = work.path().join("pipelines");
    for (name, content) in &extracted.steps {
        // Built-in steps may be single-segment (`make`, `cmake`), while local
        // pipeline files must be namespaced. The digest check below still
        // pins their content.
        if crate::domain::steps::builtin_source(name).is_none() {
            validate_name(name)?;
        }
        let path = pipelines.join(format!("{name}.yaml"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::new(
                    codes::BUILD_006,
                    format!("cannot create {}: {error}", parent.display()),
                )
            })?;
        }
        std::fs::write(&path, content).map_err(|error| {
            Error::new(
                codes::BUILD_006,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }

    let locked_policy = PlanPolicy {
        dist_tag: package_lock.dist_tag.as_deref(),
        build_release: package_lock.build_release,
    };
    let policy = reconcile_policy(request.policy, locked_policy)?;

    let recipe_path = work.path().join("package.yaml");
    let (bundle, plan) = load_plan(
        &recipe_path,
        &target_arch,
        Some(pipelines.as_path()),
        true,
        false,
        policy,
    )?;
    let cells: Vec<_> = match request.input.cell.as_deref() {
        Some(id) => plan.cells.iter().filter(|cell| cell.id == id).collect(),
        None => plan.cells.iter().collect(),
    };
    if cells.is_empty() {
        return Err(Error::new(
            codes::BUILD_002,
            format!(
                "no such cell '{}'",
                request.input.cell.as_deref().unwrap_or_default()
            ),
        ));
    }
    if cells.len() > 1 {
        return Err(Error::new(
            codes::BUILD_003,
            "this plan has several cells; pass --cell for each rebuild",
        ));
    }
    if cells[0].version != package_lock.version || cells[0].release != package_lock.release {
        return Err(Error::new(
            codes::SRPM_012,
            format!(
                "reconstructed {}-{}-{} does not match the SRPM lock's {}-{}-{}",
                cells[0].package,
                cells[0].version,
                cells[0].release,
                cells[0].package,
                package_lock.version,
                package_lock.release
            ),
        ));
    }

    let selected_cell = cells[0].clone();
    if !request.config.quiet {
        crate::domain::output::phase(
            "prepare",
            format!(
                "{} {}-{} · {}",
                selected_cell.package, selected_cell.version, selected_cell.release, request.arch
            ),
        );
    }

    let stage = work.path().join("stage");
    std::fs::create_dir_all(&stage).map_err(|error| {
        Error::new(
            codes::BUILD_006,
            format!("cannot create {}: {error}", stage.display()),
        )
    })?;

    let mut locked_packages: BTreeSet<String> = BTreeSet::new();
    let embedded_lock = {
        let path = work.path().join("root.lock.json");
        if path.exists() {
            Some(load_lock(&path)?)
        } else {
            None
        }
    };
    let requested_lock = match request.config.root.lock() {
        Some(path) => Some(load_lock(path)?),
        None => None,
    };
    let root_differs = match (&requested_lock, &embedded_lock) {
        (Some(requested), Some(embedded)) => {
            requested.closure_digest != embedded.closure_digest
                || requested.root_digest != embedded.root_digest
        }
        _ => false,
    };
    if let (Some(requested), Some(embedded)) = (&requested_lock, &embedded_lock) {
        if root_differs && !request.input.replace_root_lock {
            return Err(Error::new(
                codes::ROOT_016,
                "--root-lock differs from the root lock embedded in the SRPM \
                 (closure or root digest)",
            ));
        }
        if root_differs && request.input.replace_root_lock {
            crate::domain::output::warning(
                "bootstrap",
                format!(
                    "replacing embedded root lock {} with {}",
                    embedded.root_digest.as_deref().unwrap_or("<none>"),
                    requested.root_digest.as_deref().unwrap_or("<none>")
                ),
            );
        }
    }
    let replaced_root = if root_differs {
        embedded_lock.clone()
    } else {
        None
    };
    let allow_appliance_resolve = requested_lock.is_some();
    let active_lock = requested_lock.or(embedded_lock);
    if let Some(lock) = &active_lock {
        crate::support::autoroot::report_root(lock, &lock.profile, "from lock");
    }
    // A cross SRPM records its toolchain and sysroot, so rebuild runs the
    // compiler on this host instead of executing the target's.
    let exec = if embedded_cross.is_some() {
        crate::domain::Arch::current()?
    } else {
        crate::domain::Arch::parse(&target_arch)?
    };

    let mut assembled_appliance: Option<PathBuf> = None;
    let assembled_root: Option<RootSelection> = if let Some(lock) = &active_lock {
        for entries in lock.packages.values() {
            for package in entries {
                locked_packages.insert(package.name.clone());
            }
        }
        let runner_kind = request
            .config
            .runner
            .as_str()
            .parse::<crate::domain::runner::RunnerKind>()?;
        // The root is where steps run: a cross build resolves and verifies a
        // host-architecture build root, not the target root.
        let exec_name = exec.as_str();
        let needs_appliance = crate::domain::appliance::Policy::Auto.should_resolve(
            crate::domain::Arch::current()?,
            exec,
            runner_kind,
        );
        let selection = if request.prepared.is_some() {
            assembled_appliance = request.prepared.and_then(|env| env.appliance.clone());
            None
        } else {
            let path = match request.config.root.path() {
                Some(root_path) => root_path.to_path_buf(),
                None => {
                    let roots = crate::autoroot::ensure_from_lock(
                        lock,
                        request.config.cache.as_deref(),
                        request.config.offline,
                        exec_name,
                        needs_appliance,
                        request.config.runner_assets.as_deref(),
                        allow_appliance_resolve,
                        embedded_cross.is_some(),
                    )?;
                    assembled_appliance = roots.appliance;
                    roots.root
                }
            };
            Some(RootSelection::verify(lock, &path, exec_name)?)
        };
        if request.prepared.is_none() && request.config.root.path().is_some() {
            assembled_appliance = crate::support::autoroot::appliance_for(
                lock,
                request.config.cache.as_deref(),
                request.config.offline,
                runner_kind,
                request.arch,
                request.config.runner_assets.as_deref(),
                allow_appliance_resolve,
            )?;
        }
        selection
    } else {
        if request.config.root.path().is_some() {
            return Err(Error::new(
                codes::ROOT_016,
                "--root requires a root lock: pass --root-lock or use an SRPM with root.lock.json",
            ));
        }
        if !request.config.root.unsafe_host() {
            return Err(Error::new(codes::ROOT_016, "no root available: the SRPM has no embedded root lock and no explicit lock was selected"));
        }
        None
    };

    let replaced_sysroot = cross_lock_and_arch.as_ref().map(|(lock, _)| lock.clone());
    let (sysroot, cross, sysroot_lock) = match (&embedded_cross, cross_lock_and_arch) {
        (Some(recorded), Some((lock, arch))) => {
            let root = if let Some(root) = request.prepared.and_then(|env| env.sysroot.as_ref()) {
                root.path().to_path_buf()
            } else {
                crate::support::autoroot::ensure_sysroot_from_lock(
                    &lock,
                    request.config.cache.as_deref(),
                    request.config.offline,
                    &arch,
                )?
            };
            let mut toolchain = if recorded.native {
                crate::domain::guest::CrossToolchain::native(&recorded.triple)
            } else {
                crate::domain::guest::CrossToolchain::new(&recorded.triple)
            };
            toolchain.cflags = recorded.cflags.clone();
            toolchain.cxxflags = recorded.cxxflags.clone();
            toolchain.ldflags = recorded.ldflags.clone();
            (Some(root), Some(toolchain), Some(lock))
        }
        _ => (None, None, None),
    };

    let replaced = if request.input.replace_root_lock {
        let root = replaced_root.as_ref().map(lock_identity);
        let cross = replaced_sysroot.as_ref().map(lock_identity);
        if root.is_none() && cross.is_none() {
            None
        } else {
            Some(crate::domain::srpm::ReplacedLock { root, cross })
        }
    } else {
        None
    };

    let recipe = bundle.recipe.clone();
    Ok(RebuildState {
        work,
        extracted,
        package_lock,
        bundled,
        bundle,
        recipe,
        plan,
        cell: selected_cell,
        pipelines,
        build,
        stage,
        active_lock,
        locked_packages,
        assembled_root,
        assembled_appliance,
        signature_timestamp,
        exec,
        sysroot,
        cross,
        sysroot_lock,
        target: target_arch,
        replaced,
    })
}

fn lock_identity(lock: &crate::domain::root::RootLock) -> crate::domain::srpm::LockIdentity {
    crate::domain::srpm::LockIdentity {
        closure_digest: lock.closure_digest.clone(),
        root_digest: lock.root_digest.clone(),
        profile: Some(lock.profile.clone()),
    }
}
