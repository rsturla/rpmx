use std::collections::BTreeSet;
use std::path::PathBuf;

use rpmx_core::error::codes;
use rpmx_core::root::{load_lock, RootSelection};
use rpmx_core::srpm::load_lock_file;
use rpmx_core::steps::validate_name;
use rpmx_core::Error;
use rpmx_rpm::extract::extract_srpm;

use crate::commands::{load_plan, reconcile_policy, PlanPolicy};

use super::RebuildRequest;

pub(super) struct RebuildState {
    pub(super) work: crate::commands::WorkDir,
    pub(super) extracted: rpmx_rpm::extract::ExtractedSrpm,
    pub(super) package_lock: rpmx_core::srpm::PackageLock,
    pub(super) recipe: rpmx_core::Recipe,
    pub(super) cell: rpmx_core::plan::PlannedCell,
    pub(super) build: PathBuf,
    pub(super) stage: PathBuf,
    pub(super) active_lock: Option<rpmx_core::root::RootLock>,
    pub(super) builder_image: Option<String>,
    pub(super) locked_packages: BTreeSet<String>,
    pub(super) assembled_root: Option<PathBuf>,
    pub(super) assembled_appliance: Option<PathBuf>,
    pub(super) signature_timestamp: Option<u32>,
}

pub(super) fn prepare(
    request: &RebuildRequest<'_>,
    signing: Option<&rpmx_rpm::sign::SigningKey>,
) -> Result<RebuildState, Error> {
    let work = crate::commands::WorkDir::new("rebuild")?;
    rpmx_core::output::phase("prepare", format!("reading {}", request.file.display()));
    let extracted = extract_srpm(request.file, work.path())?;
    // Read the authoritative lock before unpacking the snapshot. The snapshot
    // is user-controlled recipe data and must not be able to replace it.
    let package_lock = load_lock_file(&work.path().join("package.lock.json"))?;
    let signature_timestamp = crate::commands::signature_timestamp(
        request.signing.signing_timestamp,
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
        rpmx_core::source::unpack_snapshot(&snapshot, &build)?;
    }

    let pipelines = work.path().join("pipelines");
    for (name, content) in &extracted.steps {
        // Built-in steps may be single-segment (`make`, `cmake`), while local
        // pipeline files must be namespaced. The digest check below still
        // pins their content.
        if rpmx_core::steps::builtin_source(name).is_none() {
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
        request.arch,
        Some(pipelines.as_path()),
        true,
        policy,
    )?;
    let cells: Vec<_> = match request.cell {
        Some(id) => plan.cells.iter().filter(|cell| cell.id == id).collect(),
        None => plan.cells.iter().collect(),
    };
    if cells.is_empty() {
        return Err(Error::new(
            codes::BUILD_002,
            format!("no such cell '{}'", request.cell.unwrap_or_default()),
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
    rpmx_core::output::phase(
        "prepare",
        format!(
            "{}-{}-{} {} ({} build steps)",
            selected_cell.package,
            selected_cell.version,
            selected_cell.release,
            request.arch,
            selected_cell.pipeline.len()
        ),
    );

    let stage = work.path().join("stage");
    std::fs::create_dir_all(&stage).map_err(|error| {
        Error::new(
            codes::BUILD_006,
            format!("cannot create {}: {error}", stage.display()),
        )
    })?;

    let mut locked_packages: BTreeSet<String> = BTreeSet::new();
    let mut builder_image: Option<String> = request.root_image.map(str::to_string);
    let embedded_lock = {
        let path = work.path().join("root.lock.json");
        if path.exists() {
            Some(load_lock(&path)?)
        } else {
            None
        }
    };
    let requested_lock = match request.root_lock {
        Some(path) => Some(load_lock(path)?),
        None => None,
    };
    if let (Some(requested), Some(embedded)) = (&requested_lock, &embedded_lock) {
        if requested.closure_digest != embedded.closure_digest
            || requested.root_digest != embedded.root_digest
            || requested.debuginfo != embedded.debuginfo
        {
            return Err(Error::new(
                codes::ROOT_016,
                "--root-lock differs from the root lock embedded in the SRPM \
                 (closure or root digest)",
            ));
        }
    }
    let active_lock = requested_lock.or(embedded_lock);
    if let Some(lock) = &active_lock {
        crate::commands::autoroot::report_root(lock, &lock.profile, "locked");
    }

    let mut assembled_appliance: Option<PathBuf> = None;
    let assembled_root: Option<PathBuf> = if let Some(lock) = &active_lock {
        if request.root_image.is_some() {
            return Err(Error::new(
                codes::ROOT_016,
                "a root lock cannot be combined with --root-image",
            ));
        }
        builder_image = lock.builder.as_ref().map(|builder| builder.image.clone());
        for entries in lock.packages.values() {
            for package in entries {
                locked_packages.insert(package.name.clone());
            }
        }
        let runner_kind = request.runner.parse::<rpmx_core::runner::RunnerKind>()?;
        let needs_appliance = rpmx_core::appliance::Policy::Auto.should_resolve(
            rpmx_core::Arch::current()?,
            rpmx_core::Arch::parse(request.arch)?,
            runner_kind,
        );
        let path = match request.root {
            Some(root_path) if request.verify_root => {
                RootSelection::verify(lock, root_path, request.arch)?.path
            }
            Some(root_path) => {
                lock.require_target(request.arch)?;
                lock.require_digest()?;
                std::fs::canonicalize(root_path).map_err(|error| {
                    Error::new(
                        codes::ROOT_017,
                        format!("cannot resolve {}: {error}", root_path.display()),
                    )
                })?
            }
            None => {
                let roots = crate::commands::autoroot::ensure_from_lock(
                    lock,
                    request.cache,
                    request.offline,
                    request.arch,
                    needs_appliance,
                    request.runner_asset_root,
                )?;
                assembled_appliance = roots.appliance;
                roots.root
            }
        };
        if request.root.is_some() {
            assembled_appliance = crate::commands::autoroot::appliance_for(
                lock,
                request.cache,
                request.offline,
                runner_kind,
                request.arch,
                request.runner_asset_root,
            )?;
        }
        Some(path)
    } else {
        if request.root.is_some() {
            return Err(Error::new(
                codes::ROOT_016,
                "--root requires a root lock: pass --root-lock or use an SRPM with root.lock.json",
            ));
        }
        if request.root_image.is_none() {
            return Err(Error::new(
                codes::ROOT_016,
                "no root available: the SRPM has no root.lock.json and --root-lock was not given; \
                 pass --root-lock, or --root-image for the image backend",
            ));
        }
        None
    };

    Ok(RebuildState {
        work,
        extracted,
        package_lock,
        recipe: bundle.recipe,
        cell: selected_cell,
        build,
        stage,
        active_lock,
        builder_image,
        locked_packages,
        assembled_root,
        assembled_appliance,
        signature_timestamp,
    })
}
