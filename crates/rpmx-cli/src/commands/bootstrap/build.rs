use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::profile::{load_profile, Profile, Repository};
use rpmx_core::root::{
    assemble, assemble_sysroot, load_lock, resolve_profile, write_lock, AssembleOptions, RootLock,
    RootSelection,
};
use rpmx_core::{Arch, Error};

use crate::commands::{
    autoroot, fetch::default_cache, load_plan, pipeline::BuildPipeline, resolve_policy,
    BootstrapBuildArgs, PolicyInput,
};

pub fn run(args: BootstrapBuildArgs) -> Result<(), Error> {
    let plan = rpmx_core::bootstrap::plan_manifest(&args.manifest, args.arch.as_deref())?;
    let arch = select_arch(&plan, args.arch.as_deref())?;
    let base = manifest_base(&args.manifest)?;
    let stages: Vec<_> = plan
        .stages
        .iter()
        .filter(|stage| stage.arch == arch)
        .collect();
    let selected = select_stages(&stages, args.stage.as_deref())?;
    let cache = match args.runtime.cache.cache.clone() {
        Some(path) => path,
        None => default_cache()?,
    };
    std::fs::create_dir_all(&cache).map_err(|error| {
        Error::new(
            codes::BOOTSTRAP_007,
            format!("cannot create cache {}: {error}", cache.display()),
        )
    })?;
    let signing = crate::commands::load_signing_key(
        args.signing.signing_key.as_deref(),
        args.signing.signing_passphrase_env.as_deref(),
    )?;

    for stage in selected {
        check_inputs(stage, &stages, &base, arch, &args.manifest)?;
        build_stage(
            &plan,
            &stages,
            stage,
            &base,
            arch,
            &cache,
            signing.as_ref(),
            &args,
        )?;
    }
    Ok(())
}

fn select_arch<'a>(
    plan: &'a rpmx_core::BootstrapPlan,
    requested: Option<&str>,
) -> Result<&'a str, Error> {
    if plan.archs.len() > 1 && requested.is_none() {
        return Err(Error::new(
            codes::BOOTSTRAP_009,
            format!(
                "this manifest declares several architectures; pass --arch with one of: {}",
                plan.archs.join(", ")
            ),
        ));
    }
    plan.archs.first().map(String::as_str).ok_or_else(|| {
        Error::new(
            codes::BOOTSTRAP_009,
            "the manifest produced no architecture; add archs or pass --arch",
        )
    })
}

fn select_stages<'a>(
    stages: &'a [&'a rpmx_core::PlannedStage],
    requested: Option<&str>,
) -> Result<Vec<&'a rpmx_core::PlannedStage>, Error> {
    match requested {
        None => Ok(stages.to_vec()),
        Some(name) => stages
            .iter()
            .copied()
            .find(|stage| stage.name == name)
            .map(|stage| vec![stage])
            .ok_or_else(|| {
                let available = stages
                    .iter()
                    .map(|stage| stage.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Error::new(
                    codes::BOOTSTRAP_009,
                    format!("no such bootstrap stage '{name}'; available stages: {available}"),
                )
            }),
    }
}

fn manifest_base(path: &Path) -> Result<PathBuf, Error> {
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    if base.as_os_str().is_empty() {
        return std::env::current_dir().map_err(|error| {
            Error::new(
                codes::BOOTSTRAP_002,
                format!("cannot determine the current directory: {error}"),
            )
        });
    }
    Ok(std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf()))
}

fn check_inputs(
    stage: &rpmx_core::PlannedStage,
    stages: &[&rpmx_core::PlannedStage],
    base: &Path,
    arch: &str,
    manifest: &Path,
) -> Result<(), Error> {
    for input in &stage.inputs {
        let Some(dependency) = stages.iter().find(|candidate| candidate.name == *input) else {
            return Err(Error::new(
                codes::BOOTSTRAP_007,
                format!(
                    "stage '{}' needs input '{input}', which is not in this plan",
                    stage.name
                ),
            ));
        };
        let output = base.join(&dependency.output).join(arch);
        if !output.join("repodata/repomd.xml").is_file() {
            return Err(Error::new(
                codes::BOOTSTRAP_007,
                format!(
                    "stage '{}' needs input '{}' but {} has no repository index; build it first: \
                     rpmx bootstrap build {} --stage {} --arch {arch}",
                    stage.name,
                    dependency.name,
                    output.display(),
                    manifest.display(),
                    dependency.name
                ),
            ));
        }
        let lock = base.join(&dependency.lock);
        if !lock.is_file() {
            return Err(Error::new(
                codes::BOOTSTRAP_007,
                format!(
                    "stage '{}' needs input '{}' but its lock {} is missing; build it first",
                    stage.name,
                    dependency.name,
                    lock.display()
                ),
            ));
        }
    }
    Ok(())
}

struct StageRoots {
    build_lock_path: PathBuf,
    build_root: PathBuf,
    build_lock: RootLock,
    sysroot_root: Option<PathBuf>,
    sysroot_lock: Option<RootLock>,
    cross: Option<rpmx_core::guest::CrossToolchain>,
    exec: Arch,
    sysroot_packages: Option<BTreeSet<String>>,
}

#[allow(clippy::too_many_arguments)]
fn build_stage(
    plan: &rpmx_core::BootstrapPlan,
    stages: &[&rpmx_core::PlannedStage],
    stage: &rpmx_core::PlannedStage,
    base: &Path,
    arch: &str,
    cache: &Path,
    signing: Option<&rpmx_rpm::sign::SigningKey>,
    args: &BootstrapBuildArgs,
) -> Result<(), Error> {
    let profile_path = base.join(&stage.profile);
    let effective = synthesize_profile(plan, stages, stage, arch, base)?;
    let requirement = profile_requirement(&effective)?;
    let resolved_policy = resolve_policy(PolicyInput {
        profile: Some(&profile_path),
        dist_tag: None,
        build_release: None,
    })?;

    let roots = resolve_roots(
        &effective,
        &requirement,
        stage,
        plan,
        base,
        arch,
        cache,
        args,
    )?;

    let output = base.join(&stage.output).join(arch);
    let temp_output = temporary_sibling(&output, "stage-output")?;
    prepare_output(&output, &temp_output, args.force)?;

    let epoch = stage.source_date_epoch.unwrap_or(0);
    let result = (|| {
        let count = build_recipes(
            stage,
            &stage.recipes,
            &temp_output,
            arch,
            cache,
            &profile_path,
            resolved_policy.plan_policy(),
            &roots,
            args,
            &args.signing,
        )?;
        if count == 0 {
            return Err(Error::new(
                codes::BOOTSTRAP_003,
                format!("stage '{}' produced no RPM packages", stage.name),
            ));
        }
        index_output(&temp_output, signing, epoch)?;

        if stage.fixed_point.is_empty() {
            publish(&temp_output, &output, args.force, "stage output")?;
            return Ok::<_, Error>(count);
        }

        let (fixed_recipes, fixed_outputs) =
            fixed_recipe_outputs(stage, arch, resolved_policy.plan_policy())?;
        let mut previous_hashes = rpm_hashes(&temp_output, &fixed_outputs)?;
        let mut previous_root = root_identity(&roots.build_lock);
        let mut last_changed = Vec::new();
        let mut last_two = (previous_root.clone(), previous_root.clone());

        for iteration in 1..=plan.max_iterations {
            let package_names = rpmx_rpm::package_names(&temp_output)?;
            let iteration_profile =
                fixed_point_profile(&effective, stage, &temp_output, &package_names)?;
            let iteration_requirement = profile_requirement(&iteration_profile)?;
            let candidate = resolve_candidate_roots(
                &iteration_profile,
                &iteration_requirement,
                stage,
                base,
                arch,
                cache,
                args,
                iteration,
                &plan.seed,
            )?;
            remove_fixed_rpms(&temp_output, &fixed_outputs)?;
            build_recipes(
                stage,
                &fixed_recipes,
                &temp_output,
                arch,
                cache,
                &profile_path,
                resolved_policy.plan_policy(),
                &candidate,
                args,
                &args.signing,
            )?;
            index_output(&temp_output, signing, epoch)?;
            let hashes = rpm_hashes(&temp_output, &fixed_outputs)?;
            let current_root = root_identity(&candidate.build_lock);
            let changed = changed_packages(&previous_hashes, &hashes);
            if changed.is_empty() && current_root == previous_root {
                let final_profile =
                    fixed_point_profile(&effective, stage, &output, &package_names)?;
                let final_requirement = profile_requirement(&final_profile)?;
                promote_candidate(&candidate, &roots, &output, stage, &final_requirement)?;
                publish(&temp_output, &output, args.force, "stage output")?;
                return Ok::<_, Error>(hashes.len());
            }
            last_changed = changed;
            last_two = (previous_root.clone(), current_root.clone());
            previous_hashes = hashes;
            previous_root = current_root;
        }

        let changed = if last_changed.is_empty() {
            "none (root digests changed)".to_string()
        } else {
            last_changed.join(", ")
        };
        Err(Error::new(
            codes::BOOTSTRAP_005,
            format!(
                "stage '{}': fixed point did not converge after {} iteration(s); changed \
                 packages: {changed}; last root digests: {}/{} -> {}/{}",
                stage.name,
                plan.max_iterations,
                last_two.0 .0,
                last_two.0 .1,
                last_two.1 .0,
                last_two.1 .1
            ),
        ))
    })();

    let count = match result {
        Ok(count) => count,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp_output);
            return Err(error);
        }
    };
    let final_digest = load_lock(&roots.build_lock_path)
        .ok()
        .and_then(|lock| lock.root_digest)
        .or(roots.build_lock.root_digest.clone())
        .unwrap_or_else(|| "<none>".to_string());
    rpmx_core::output::phase(
        "bootstrap",
        format!(
            "{} · {arch} · {count} package(s) · root {} · repo {} · seed {}{}",
            stage.name,
            final_digest,
            output.display(),
            plan.seed
                .as_ref()
                .and_then(|seed| seed.name.as_deref())
                .unwrap_or("-"),
            stage
                .source_date_epoch
                .map(|epoch| format!(" · epoch {epoch}"))
                .unwrap_or_default()
        ),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_recipes(
    stage: &rpmx_core::PlannedStage,
    recipe_paths: &[String],
    output: &Path,
    arch: &str,
    cache: &Path,
    profile_path: &Path,
    policy: crate::commands::PlanPolicy<'_>,
    roots: &StageRoots,
    args: &BootstrapBuildArgs,
    signing: &crate::commands::OptionalSigningArgs,
) -> Result<usize, Error> {
    let mut count = 0usize;
    for recipe_path in recipe_paths {
        let (_, recipe_plan) = load_plan(Path::new(recipe_path), arch, None, false, false, policy)?;
        let cells: Vec<_> = recipe_plan.cells.iter().collect();
        if cells.is_empty() {
            return Err(Error::new(
                codes::BOOTSTRAP_003,
                format!(
                    "stage '{}': recipe {recipe_path} has no build cells",
                    stage.name
                ),
            ));
        }
        let pipeline = BuildPipeline {
            file: Path::new(recipe_path),
            output,
            arch,
            exec: roots.exec,
            sysroot: roots.sysroot_root.as_deref(),
            cross: roots.cross.as_ref(),
            sysroot_packages: roots.sysroot_packages.as_ref(),
            sysroot_lock: roots.sysroot_lock.as_ref(),
            runner: &args.runner,
            policy,
            profile: Some(profile_path),
            cache: Some(cache),
            proxy: args.proxy.as_deref(),
            offline: args.offline,
            root_lock: Some(&roots.build_lock_path),
            root: Some(&roots.build_root),
            runner_asset_root: None,
            unsafe_host_root: false,
            source_date_epoch: stage.source_date_epoch,
            jobs: args.runtime.jobs,
            step_timeout: args.runtime.step_timeout,
            run_tests: !args.no_tests,
            skip_installed: args.skip_installed,
            save_test_root: None,
            test_filter: &[],
            junit: None,
            no_lint: false,
            sbom: None,
            test_root_lock: None,
            signing,
            pipeline_dir: None,
        };
        count += pipeline.execute(&cells)?.rpms.len();
    }
    Ok(count)
}

fn fixed_recipe_outputs(
    stage: &rpmx_core::PlannedStage,
    arch: &str,
    policy: crate::commands::PlanPolicy<'_>,
) -> Result<(Vec<String>, BTreeSet<String>), Error> {
    let fixed: BTreeSet<&str> = stage.fixed_point.iter().map(String::as_str).collect();
    let mut recipes = Vec::new();
    let mut outputs = BTreeSet::new();
    for (name, recipe) in stage.selected_recipes() {
        if !fixed.contains(name) {
            continue;
        }
        recipes.push(recipe.to_string());
        outputs.insert(name.to_string());
        let (_, plan) = load_plan(Path::new(recipe), arch, None, false, false, policy)?;
        for cell in plan.cells {
            outputs.insert(cell.package);
            for output in cell.outputs {
                outputs.insert(output.name);
            }
        }
    }
    if recipes.is_empty() {
        return Err(Error::new(
            codes::BOOTSTRAP_002,
            format!(
                "stage '{}' lists fixed-point packages that have no selected recipe",
                stage.name
            ),
        ));
    }
    Ok((recipes, outputs))
}

fn file_url(path: &Path) -> Result<String, Error> {
    let absolute = std::path::absolute(path).map_err(|error| {
        Error::new(
            codes::BOOTSTRAP_001,
            format!("cannot make {} absolute: {error}", path.display()),
        )
    })?;
    Ok(format!("file://{}", absolute.display()))
}

fn fixed_point_profile(
    effective: &Profile,
    stage: &rpmx_core::PlannedStage,
    output: &Path,
    package_names: &BTreeSet<String>,
) -> Result<Profile, Error> {
    let output_id = format!("bootstrap-{}-output", stage.name);
    if effective
        .repositories
        .iter()
        .any(|repository| repository.id == output_id)
    {
        return Err(Error::new(
            codes::BOOTSTRAP_002,
            format!(
                "stage '{}': fixed-point repository id '{output_id}' collides with a profile \
                 repository",
                stage.name
            ),
        ));
    }
    let mut profile = effective.clone();
    let mut excluded = package_names.clone();
    for (package, seeds) in &stage.replaces {
        if package_names.contains(package) || stage.packages.contains(package) {
            excluded.extend(seeds.iter().cloned());
        }
    }
    // Hide every other provider of a package this iteration builds, including
    // the seed repositories. The output repository is the only one allowed to
    // supply it, so the bootstrap does not depend on an artificial EVR bump to
    // win a version comparison against Fedora.
    for repository in &mut profile.repositories {
        if repository.id != output_id {
            repository.exclude.extend(excluded.iter().cloned());
            dedupe(&mut repository.exclude);
        }
    }
    profile.repositories.push(Repository {
        id: output_id,
        baseurl: Some(file_url(output)?),
        metalink: None,
        mirrorlist: None,
        gpgkey: None,
        repo_gpgcheck: false,
        exclude: Vec::new(),
        auth: None,
    });
    Ok(profile)
}

#[allow(clippy::too_many_arguments)]
fn resolve_candidate_roots(
    profile: &Profile,
    requirement: &str,
    stage: &rpmx_core::PlannedStage,
    base: &Path,
    arch: &str,
    cache: &Path,
    args: &BootstrapBuildArgs,
    iteration: u32,
    seed: &Option<rpmx_core::bootstrap::Seed>,
) -> Result<StageRoots, Error> {
    let mut candidate_args = args.clone();
    candidate_args.locked = false;
    candidate_args.refresh = true;
    candidate_args.force = true;

    let target = Arch::parse(arch)?;
    let crossing = profile.cross.is_some();
    let build_profile = if crossing {
        profile.build_root_profile()?
    } else {
        profile.clone()
    };
    let sysroot_profile = if crossing {
        Some(profile.sysroot_profile()?)
    } else {
        None
    };
    let build_arch = Arch::parse(build_profile.arch())?;
    let stage_root = with_suffix(&base.join(&stage.root), &format!("-iter{iteration}"));
    let build_root = stage_root.join("root");
    let build_lock_path = lock_with_suffix(&base.join(&stage.lock), &format!("-iter{iteration}"));
    let build_lock = resolve_one_root(
        &build_profile,
        &format!("{}-{}-iter{iteration}", stage.name, stage.name),
        &build_root,
        &build_lock_path,
        requirement,
        build_arch,
        false,
        seed,
        cache,
        &candidate_args,
        &stage.name,
    )?;

    let (sysroot_root, sysroot_lock) = match &sysroot_profile {
        Some(sysroot_profile) => {
            let sysroot_root = with_suffix(&stage_root, "-sysroot").join("root");
            let sysroot_lock_path = sysroot_lock_path(&build_lock_path);
            let lock = resolve_one_root(
                sysroot_profile,
                &format!("{}-iter{iteration}-sysroot", stage.name),
                &sysroot_root,
                &sysroot_lock_path,
                requirement,
                target,
                true,
                seed,
                cache,
                &candidate_args,
                &stage.name,
            )?;
            (Some(sysroot_root), Some(lock))
        }
        None => (None, None),
    };

    let sysroot_packages = sysroot_lock.as_ref().map(|lock| {
        lock.packages
            .values()
            .flatten()
            .map(|package| package.name.clone())
            .collect()
    });
    let cross = match &profile.cross {
        Some(cross) => {
            let triple = profile.triple()?;
            let mut toolchain = if cross.bootstrap && cross.triple.is_none() {
                rpmx_core::guest::CrossToolchain::native(triple)
            } else {
                rpmx_core::guest::CrossToolchain::new(triple)
            };
            toolchain.cflags = cross.cflags.clone();
            toolchain.cxxflags = cross.cxxflags.clone();
            toolchain.ldflags = cross.ldflags.clone();
            Some(toolchain)
        }
        None => None,
    };

    Ok(StageRoots {
        build_lock_path,
        build_root,
        build_lock,
        sysroot_root,
        sysroot_lock,
        cross,
        exec: if crossing { Arch::current()? } else { target },
        sysroot_packages,
    })
}

fn root_identity(lock: &RootLock) -> (String, String) {
    (
        lock.closure_digest.clone(),
        lock.root_digest.clone().unwrap_or_default(),
    )
}

fn rpm_hashes(output: &Path, wanted: &BTreeSet<String>) -> Result<BTreeMap<String, String>, Error> {
    let mut hashes = BTreeMap::new();
    let entries = std::fs::read_dir(output).map_err(|error| {
        Error::new(
            codes::BOOTSTRAP_005,
            format!("cannot read {}: {error}", output.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            Error::new(
                codes::BOOTSTRAP_005,
                format!("cannot read {}: {error}", output.display()),
            )
        })?;
        let path = entry.path();
        if !path.extension().is_some_and(|extension| extension == "rpm") {
            continue;
        }
        let name = rpmx_rpm::package_name(&path)?;
        if !wanted.is_empty() && !wanted.contains(&name) {
            continue;
        }
        let digest = rpmx_core::hash::sha512_file(&path).map_err(|error| {
            Error::new(
                codes::BOOTSTRAP_005,
                format!("cannot hash {}: {error}", path.display()),
            )
        })?;
        hashes.insert(name, digest);
    }
    Ok(hashes)
}

fn changed_packages(
    previous: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changed = BTreeSet::new();
    for (name, digest) in current {
        if previous.get(name) != Some(digest) {
            changed.insert(name.clone());
        }
    }
    for name in previous.keys() {
        if !current.contains_key(name) {
            changed.insert(name.clone());
        }
    }
    changed.into_iter().collect()
}

fn remove_fixed_rpms(output: &Path, wanted: &BTreeSet<String>) -> Result<(), Error> {
    let entries = std::fs::read_dir(output).map_err(|error| {
        Error::new(
            codes::BOOTSTRAP_005,
            format!("cannot read {}: {error}", output.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            Error::new(
                codes::BOOTSTRAP_005,
                format!("cannot read {}: {error}", output.display()),
            )
        })?;
        let path = entry.path();
        if !path.extension().is_some_and(|extension| extension == "rpm") {
            continue;
        }
        let name = rpmx_rpm::package_name(&path)?;
        if wanted.contains(&name) {
            std::fs::remove_file(&path).map_err(|error| {
                Error::new(
                    codes::BOOTSTRAP_005,
                    format!("cannot remove {}: {error}", path.display()),
                )
            })?;
        }
    }
    Ok(())
}

fn index_output(
    output: &Path,
    signing: Option<&rpmx_rpm::sign::SigningKey>,
    epoch: u32,
) -> Result<(), Error> {
    let repodata = output.join("repodata");
    if repodata.exists() {
        std::fs::remove_dir_all(&repodata).map_err(|error| {
            Error::new(
                codes::BOOTSTRAP_005,
                format!("cannot remove {}: {error}", repodata.display()),
            )
        })?;
    }
    crate::commands::repo::index(output, signing, epoch)?;
    Ok(())
}

fn promote_candidate(
    candidate: &StageRoots,
    final_roots: &StageRoots,
    output: &Path,
    stage: &rpmx_core::PlannedStage,
    requirement: &str,
) -> Result<(), Error> {
    let output_id = format!("bootstrap-{}-output", stage.name);
    let output_url = file_url(output)?;
    let mut build_lock = candidate.build_lock.clone();
    update_output_repository(&mut build_lock, &output_id, &output_url);
    build_lock.requirement = Some(requirement.to_string());
    publish(
        &candidate.build_root,
        &final_roots.build_root,
        true,
        "stage root",
    )?;
    write_lock_atomic(&final_roots.build_lock_path, &build_lock)?;
    if let (Some(candidate_lock), Some(candidate_root), Some(final_root)) = (
        &candidate.sysroot_lock,
        &candidate.sysroot_root,
        &final_roots.sysroot_root,
    ) {
        let mut sysroot_lock = candidate_lock.clone();
        update_output_repository(&mut sysroot_lock, &output_id, &output_url);
        sysroot_lock.requirement = Some(requirement.to_string());
        let lock_path = sysroot_lock_path(&final_roots.build_lock_path);
        publish(candidate_root, final_root, true, "stage sysroot")?;
        write_lock_atomic(&lock_path, &sysroot_lock)?;
    }
    Ok(())
}

fn update_output_repository(lock: &mut RootLock, output_id: &str, output_url: &str) {
    for repository in &mut lock.repositories {
        if repository.id == output_id {
            repository.baseurl = Some(output_url.to_string());
        }
    }
    lock.closure_digest = rpmx_core::root::closure_digest(
        &lock.releasever,
        &lock.archs,
        &lock.repositories,
        &lock.packages,
    );
}

fn sysroot_lock_path(build_lock_path: &Path) -> PathBuf {
    lock_with_suffix(build_lock_path, ".sysroot")
}

fn lock_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let stem = name.strip_suffix(".lock.yaml").unwrap_or(&name);
    path.with_file_name(format!("{stem}{suffix}.lock.yaml"))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    path.with_file_name(format!("{name}{suffix}"))
}

fn synthesize_profile(
    plan: &rpmx_core::BootstrapPlan,
    stages: &[&rpmx_core::PlannedStage],
    stage: &rpmx_core::PlannedStage,
    arch: &str,
    base: &Path,
) -> Result<Profile, Error> {
    let path = base.join(&stage.profile);
    let mut profile = load_profile(&path)?.for_target(arch)?;
    profile.name = Some(format!("{}-{}", plan.name, stage.name));

    // When a package keeps a different name in the seed, hide the seed
    // provider as soon as a later stage can satisfy the dependency from an
    // earlier Eternal stage. The seed stage itself must keep the seed package.
    if !stage.seed {
        let mut replaced = BTreeSet::new();
        for seeds in stage.replaces.values() {
            replaced.extend(seeds.iter().cloned());
        }
        for repository in &mut profile.repositories {
            repository.exclude.extend(replaced.iter().cloned());
            dedupe(&mut repository.exclude);
        }
    }

    for input in &stage.inputs {
        let dependency = stages
            .iter()
            .find(|candidate| candidate.name == *input)
            .ok_or_else(|| {
                Error::new(
                    codes::BOOTSTRAP_002,
                    format!("stage '{}' names unknown input '{input}'", stage.name),
                )
            })?;
        let id = format!("bootstrap-{}-{}", stage.name, input);
        if profile
            .repositories
            .iter()
            .any(|repository| repository.id == id)
        {
            return Err(Error::new(
                codes::BOOTSTRAP_002,
                format!(
                    "stage '{}': input repository id '{id}' collides with a profile repository",
                    stage.name
                ),
            ));
        }
        let output = base.join(&dependency.output).join(arch);
        profile.repositories.push(Repository {
            id,
            baseurl: Some(file_url(&output)?),
            metalink: None,
            mirrorlist: None,
            gpgkey: None,
            repo_gpgcheck: false,
            exclude: Vec::new(),
            auth: None,
        });
    }

    let mut build_extra: BTreeSet<String> = stage.extra_tools.iter().cloned().collect();
    let mut target_extra: BTreeSet<String> = stage.extra_sysroot.iter().cloned().collect();
    for recipe_path in &stage.recipes {
        let recipe = rpmx_core::load_recipe(recipe_path)?;
        let recipe_plan = rpmx_core::plan(&recipe, arch)?;
        let required = autoroot::required_packages(&recipe_plan);
        build_extra.extend(required.build);
        target_extra.extend(required.target);
    }

    if let Some(cross) = profile.cross.as_mut() {
        profile.packages.install.extend(target_extra);
        cross.packages.extend(build_extra);
        dedupe(&mut profile.packages.install);
        dedupe(&mut cross.packages);
    } else {
        profile.packages.install.extend(build_extra);
        profile.packages.install.extend(target_extra);
        dedupe(&mut profile.packages.install);
    }
    Ok(profile)
}

fn dedupe(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn profile_requirement(profile: &Profile) -> Result<String, Error> {
    let text = serde_yaml_ng::to_string(profile)
        .map_err(|error| Error::new(codes::BOOTSTRAP_002, error.to_string()))?;
    Ok(rpmx_core::hash::sha512_hex(text.as_bytes()))
}

#[allow(clippy::too_many_arguments)]
fn resolve_roots(
    effective: &Profile,
    requirement: &str,
    stage: &rpmx_core::PlannedStage,
    plan: &rpmx_core::BootstrapPlan,
    base: &Path,
    arch: &str,
    cache: &Path,
    args: &BootstrapBuildArgs,
) -> Result<StageRoots, Error> {
    let target = Arch::parse(arch)?;
    let crossing = effective.cross.is_some();
    let build_profile = if crossing {
        effective.build_root_profile()?
    } else {
        effective.clone()
    };
    let sysroot_profile = if crossing {
        Some(effective.sysroot_profile()?)
    } else {
        None
    };
    let build_arch = Arch::parse(build_profile.arch())?;

    let stage_root = base.join(&stage.root);
    let build_root = stage_root.join("root");
    let build_lock_path = base.join(&stage.lock);
    let seed = plan.seed.clone();

    let build_lock = resolve_one_root(
        &build_profile,
        &format!("{}-{}", plan.name, stage.name),
        &build_root,
        &build_lock_path,
        requirement,
        build_arch,
        false,
        &seed,
        cache,
        args,
        &stage.name,
    )?;

    let (sysroot_root, sysroot_lock) = match &sysroot_profile {
        Some(profile) => {
            let sysroot_root = with_suffix(&stage_root, "-sysroot").join("root");
            let sysroot_lock_path = sysroot_lock_path(&build_lock_path);
            let lock = resolve_one_root(
                profile,
                &format!("{}-{}-sysroot", plan.name, stage.name),
                &sysroot_root,
                &sysroot_lock_path,
                requirement,
                target,
                true,
                &seed,
                cache,
                args,
                &stage.name,
            )?;
            (Some(sysroot_root), Some(lock))
        }
        None => (None, None),
    };

    let sysroot_packages = sysroot_lock.as_ref().map(|lock| {
        lock.packages
            .values()
            .flatten()
            .map(|package| package.name.clone())
            .collect()
    });
    let cross = match &effective.cross {
        Some(cross) => {
            let triple = effective.triple()?;
            let mut toolchain = if cross.bootstrap && cross.triple.is_none() {
                rpmx_core::guest::CrossToolchain::native(triple)
            } else {
                rpmx_core::guest::CrossToolchain::new(triple)
            };
            toolchain.cflags = cross.cflags.clone();
            toolchain.cxxflags = cross.cxxflags.clone();
            toolchain.ldflags = cross.ldflags.clone();
            Some(toolchain)
        }
        None => None,
    };

    Ok(StageRoots {
        build_lock_path,
        build_root,
        build_lock,
        sysroot_root,
        sysroot_lock,
        cross,
        exec: if crossing { Arch::current()? } else { target },
        sysroot_packages,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_one_root(
    profile: &Profile,
    name: &str,
    root: &Path,
    lock_path: &Path,
    requirement: &str,
    expected_arch: Arch,
    sysroot: bool,
    seed: &Option<rpmx_core::bootstrap::Seed>,
    cache: &Path,
    args: &BootstrapBuildArgs,
    stage: &str,
) -> Result<RootLock, Error> {
    let arch = expected_arch.as_str();
    if args.locked {
        return locked_root(stage, root, lock_path, requirement, arch);
    }

    if !args.refresh && !args.force && lock_path.is_file() && root.is_dir() {
        if let Ok(lock) = load_lock(lock_path) {
            if lock.requirement.as_deref() == Some(requirement) {
                if let Ok(selection) = RootSelection::verify(&lock, root, arch) {
                    return Ok(selection.lock);
                }
            }
        }
    }

    if args.offline {
        return Err(Error::new(
            codes::ROOT_016,
            format!(
                "stage '{stage}' root {arch} is not cached; run `rpmx bootstrap build {}` \
                 without --offline to resolve it",
                args.manifest.display()
            ),
        ));
    }

    if root.exists() {
        ensure_replaceable(root, "stage root", args.force)?;
    }

    let resolve_work = crate::commands::WorkDir::new("bootstrap-resolve")?;
    let assemble_work = crate::commands::WorkDir::new("bootstrap-assemble")?;
    let temp_root = temporary_sibling(root, "root")?;
    let _ = std::fs::remove_dir_all(&temp_root);

    let mut lock = resolve_profile(profile, name, resolve_work.path(), cache)?;
    lock.seed = seed.as_ref().and_then(|seed| seed.name.clone());
    lock.generation = seed.as_ref().and_then(|seed| seed.generation.clone());
    lock.requirement = Some(requirement.to_string());

    let options = AssembleOptions {
        cache,
        work: assemble_work.path(),
        root: &temp_root,
    };
    let assembled = if sysroot {
        assemble_sysroot(&lock, options)?
    } else {
        assemble(&lock, options)?
    };
    lock.root_digest = Some(rpmx_core::tree::digest_root(&assembled)?);
    publish(&temp_root, root, args.force, "stage root")?;
    write_lock_atomic(lock_path, &lock)?;
    Ok(lock)
}

fn locked_root(
    stage: &str,
    root: &Path,
    lock_path: &Path,
    requirement: &str,
    arch: &str,
) -> Result<RootLock, Error> {
    if !lock_path.is_file() {
        return Err(Error::new(
            codes::BOOTSTRAP_006,
            format!(
                "stage '{stage}' lock {} is missing; run without --locked to create it, then \
                 commit the lock",
                lock_path.display()
            ),
        ));
    }
    let lock = load_lock(lock_path).map_err(|error| {
        Error::new(
            codes::BOOTSTRAP_006,
            format!(
                "stage '{stage}' lock {} is invalid: {}",
                lock_path.display(),
                error.message
            ),
        )
    })?;
    if lock.requirement.as_deref() != Some(requirement) {
        return Err(Error::new(
            codes::BOOTSTRAP_006,
            format!(
                "stage '{stage}' lock {} is stale: its requirement does not match the current \
                 profile and stage extras; run without --locked to update it, or --refresh to \
                 accept a new resolution",
                lock_path.display()
            ),
        ));
    }
    if !root.is_dir() {
        return Err(Error::new(
            codes::BOOTSTRAP_006,
            format!(
                "stage '{stage}' root {} is missing; run without --locked to assemble it",
                root.display()
            ),
        ));
    }
    RootSelection::verify(&lock, root, arch)
        .map(|selection| selection.lock)
        .map_err(|error| {
            Error::new(
                codes::BOOTSTRAP_006,
                format!(
                    "stage '{stage}' root {} has drifted: {}; run without --locked to rebuild it, \
                     or --refresh to accept a new resolution",
                    root.display(),
                    error.message
                ),
            )
        })
}

fn prepare_output(output: &Path, temp: &Path, force: bool) -> Result<(), Error> {
    ensure_replaceable(output, "stage output", force)?;
    let _ = std::fs::remove_dir_all(temp);
    if let Some(parent) = temp.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            Error::new(
                codes::BUILD_001,
                format!("cannot create {}: {error}", parent.display()),
            )
        })?;
    }
    Ok(())
}

fn ensure_replaceable(path: &Path, label: &str, force: bool) -> Result<(), Error> {
    if !path.exists() {
        return Ok(());
    }
    let non_empty = if path.is_dir() {
        std::fs::read_dir(path)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(true)
    } else {
        true
    };
    if non_empty && !force {
        return Err(Error::new(
            codes::BOOTSTRAP_008,
            format!(
                "{label} {} already exists; pass --force to replace it, or choose another output",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn publish(temp: &Path, destination: &Path, force: bool, label: &str) -> Result<(), Error> {
    ensure_replaceable(destination, label, force)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            Error::new(
                codes::BUILD_001,
                format!("cannot create {}: {error}", parent.display()),
            )
        })?;
    }
    let backup = destination.with_extension(format!("bootstrap-old-{}", std::process::id()));
    let old_mode = if destination.exists() {
        let _ = rpmx_core::root::remove_tree(&backup);
        let old_mode = ensure_owner_writable(destination)?;
        if let Err(error) = std::fs::rename(destination, &backup) {
            restore_mode(destination, old_mode)?;
            return Err(Error::new(
                codes::BUILD_001,
                format!("cannot move {} aside: {error}", destination.display()),
            ));
        }
        old_mode
    } else {
        None
    };
    // Assembled roots are read-only (mode 0555). Linux refuses to rename a
    // directory that its owner cannot write, so make the top-level directory
    // writable for the rename and put the original mode back afterwards.
    let temp_mode = ensure_owner_writable(temp)?;
    if let Err(error) = std::fs::rename(temp, destination) {
        restore_mode(temp, temp_mode)?;
        if backup.exists() {
            let _ = std::fs::rename(&backup, destination);
            restore_mode(destination, old_mode)?;
        }
        return Err(Error::new(
            codes::BUILD_001,
            format!(
                "cannot publish {} as {}: {error}",
                temp.display(),
                destination.display()
            ),
        ));
    }
    restore_mode(destination, temp_mode)?;
    let _ = rpmx_core::root::remove_tree(&backup);
    Ok(())
}

fn ensure_owner_writable(path: &Path) -> Result<Option<u32>, Error> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        Error::new(
            codes::BUILD_001,
            format!("cannot inspect {}: {error}", path.display()),
        )
    })?;
    if !metadata.is_dir() {
        return Ok(None);
    }
    let mode = metadata.permissions().mode() & 0o7777;
    if mode & 0o200 != 0 {
        return Ok(None);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o200)).map_err(
        |error| {
            Error::new(
                codes::BUILD_001,
                format!("cannot make {} writable: {error}", path.display()),
            )
        },
    )?;
    Ok(Some(mode))
}

fn restore_mode(path: &Path, mode: Option<u32>) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;

    let Some(mode) = mode else {
        return Ok(());
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|error| {
        Error::new(
            codes::BUILD_001,
            format!("cannot restore permissions on {}: {error}", path.display()),
        )
    })
}

fn write_lock_atomic(path: &Path, lock: &RootLock) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot create {}: {error}", parent.display()),
            )
        })?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    write_lock(&temporary, lock)?;
    std::fs::rename(&temporary, path).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot publish {}: {error}", path.display()),
        )
    })
}

fn temporary_sibling(path: &Path, kind: &str) -> Result<PathBuf, Error> {
    let name = path
        .file_name()
        .ok_or_else(|| Error::new(codes::BOOTSTRAP_002, "output path has no file name"))?
        .to_string_lossy();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    Ok(parent.join(format!(".{name}.bootstrap-{kind}-{}", std::process::id())))
}

#[cfg(test)]
mod tests {
    use super::{
        check_inputs, ensure_replaceable, publish, select_arch, select_stages, synthesize_profile,
    };
    use rpmx_core::error::codes;
    use std::path::Path;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn fixture(root: &Path) {
        write(
            &root.join("recipes/gcc/package.yaml"),
            r#"
schema: 1
package:
  name: gcc
  version: "1"
  release: 1
  summary: gcc
  license: MIT
build:
  packages: [make]
  sysroot: [zlib-devel]
outputs:
  - name: gcc
    metapackage: true
"#,
        );
        write(
            &root.join("stage0.profile.yaml"),
            r#"
document: rpmx-profile
version: 1
name: stage0
releasever: "44"
archs: [x86_64]
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
cross:
  bootstrap: true
  packages: [seed-gcc]
"#,
        );
        write(
            &root.join("stage1.profile.yaml"),
            r#"
document: rpmx-profile
version: 1
name: stage1
releasever: "44"
archs: [x86_64]
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
"#,
        );
        write(
            &root.join("bootstrap.yaml"),
            r#"
document: rpmx-bootstrap
version: 1
name: demo
archs: [x86_64]
recipe-root: recipes
source-date-epoch: 1704067200
stages:
  - name: stage0
    profile: stage0.profile.yaml
    packages: [gcc]
    output: out/stage0
    seed: true
    extra-tools: [bison]
    extra-sysroot: [zlib-devel]
  - name: stage1
    profile: stage1.profile.yaml
    packages: [gcc]
    output: out
    extra-tools: [cmake]
    extra-sysroot: [python3-devel]
"#,
        );
    }

    #[test]
    fn a_cross_stage_splits_extra_tools_and_sysroot_packages() {
        let directory = tempfile::tempdir().unwrap();
        fixture(directory.path());
        let manifest = directory.path().join("bootstrap.yaml");
        let plan = rpmx_core::bootstrap::plan_manifest(&manifest, Some("x86_64")).unwrap();
        let stages: Vec<_> = plan.stages.iter().collect();
        let profile =
            synthesize_profile(&plan, &stages, &plan.stages[0], "x86_64", directory.path())
                .unwrap();
        assert_eq!(profile.name.as_deref(), Some("demo-stage0"));
        let cross = profile.cross.as_ref().expect("cross");
        assert!(cross.packages.contains(&"bison".to_string()));
        assert!(cross.packages.contains(&"make".to_string()));
        assert!(profile.packages.install.contains(&"zlib-devel".to_string()));
        assert!(!profile.packages.install.contains(&"bison".to_string()));
    }

    #[test]
    fn a_native_stage_merges_extras_and_adds_input_repositories() {
        let directory = tempfile::tempdir().unwrap();
        fixture(directory.path());
        let manifest = directory.path().join("bootstrap.yaml");
        let plan = rpmx_core::bootstrap::plan_manifest(&manifest, Some("x86_64")).unwrap();
        let stages: Vec<_> = plan.stages.iter().collect();
        let profile =
            synthesize_profile(&plan, &stages, &plan.stages[1], "x86_64", directory.path())
                .unwrap();
        assert_eq!(profile.name.as_deref(), Some("demo-stage1"));
        assert!(profile.packages.install.contains(&"cmake".to_string()));
        assert!(profile
            .packages
            .install
            .contains(&"python3-devel".to_string()));
        assert!(profile.packages.install.contains(&"make".to_string()));
        let repo = profile
            .repositories
            .iter()
            .find(|repo| repo.id == "bootstrap-stage1-stage0")
            .expect("input repository");
        assert!(
            repo.baseurl
                .as_deref()
                .unwrap()
                .ends_with("out/stage0/x86_64"),
            "{}",
            repo.baseurl.as_deref().unwrap()
        );
    }

    #[test]
    fn a_missing_input_repository_is_a_prerequisite_error() {
        let directory = tempfile::tempdir().unwrap();
        fixture(directory.path());
        let manifest = directory.path().join("bootstrap.yaml");
        let plan = rpmx_core::bootstrap::plan_manifest(&manifest, Some("x86_64")).unwrap();
        let stages: Vec<_> = plan.stages.iter().collect();
        let error = check_inputs(
            &plan.stages[1],
            &stages,
            directory.path(),
            "x86_64",
            &manifest,
        )
        .expect_err("missing input output");
        assert_eq!(error.code, codes::BOOTSTRAP_007);
        assert!(error.message.contains("stage1"), "{}", error.message);
    }

    #[test]
    fn unknown_stage_and_ambiguous_arch_are_actionable() {
        let directory = tempfile::tempdir().unwrap();
        fixture(directory.path());
        let manifest = directory.path().join("bootstrap.yaml");
        let plan = rpmx_core::bootstrap::plan_manifest(&manifest, Some("x86_64")).unwrap();
        let stages: Vec<_> = plan.stages.iter().collect();
        let error = select_stages(&stages, Some("nope")).expect_err("unknown stage");
        assert_eq!(error.code, codes::BOOTSTRAP_009);
        assert!(error.message.contains("stage0"), "{}", error.message);

        let multi = rpmx_core::bootstrap::BootstrapPlan {
            archs: vec!["x86_64".to_string(), "aarch64".to_string()],
            ..plan
        };
        let error = select_arch(&multi, None).expect_err("ambiguous arch");
        assert_eq!(error.code, codes::BOOTSTRAP_009);
        assert!(error.message.contains("x86_64"), "{}", error.message);
    }

    #[test]
    fn changed_packages_reports_additions_removals_and_changes() {
        let previous = [
            ("gcc".to_string(), "a".to_string()),
            ("glibc".to_string(), "b".to_string()),
        ]
        .into_iter()
        .collect();
        let current = [
            ("gcc".to_string(), "c".to_string()),
            ("binutils".to_string(), "d".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            super::changed_packages(&previous, &current),
            vec![
                "binutils".to_string(),
                "gcc".to_string(),
                "glibc".to_string()
            ]
        );
        assert!(super::changed_packages(&current, &current).is_empty());
    }

    #[test]
    fn a_fixed_point_profile_excludes_input_packages_and_adds_output_repo() {
        let directory = tempfile::tempdir().unwrap();
        fixture(directory.path());
        let manifest = directory.path().join("bootstrap.yaml");
        let plan = rpmx_core::bootstrap::plan_manifest(&manifest, Some("x86_64")).unwrap();
        let stages: Vec<_> = plan.stages.iter().collect();
        let effective =
            synthesize_profile(&plan, &stages, &plan.stages[1], "x86_64", directory.path())
                .unwrap();
        let output = directory.path().join("out/x86_64");
        let names = ["gcc".to_string(), "glibc".to_string()]
            .into_iter()
            .collect();
        let iteration =
            super::fixed_point_profile(&effective, &plan.stages[1], &output, &names).unwrap();
        let input = iteration
            .repositories
            .iter()
            .find(|repo| repo.id == "bootstrap-stage1-stage0")
            .expect("input repo");
        assert_eq!(input.exclude, vec!["gcc".to_string(), "glibc".to_string()]);
        let seed = iteration
            .repositories
            .iter()
            .find(|repo| repo.id == "base")
            .expect("seed repo");
        assert_eq!(seed.exclude, vec!["gcc".to_string(), "glibc".to_string()]);
        let output_repo = iteration
            .repositories
            .iter()
            .find(|repo| repo.id == "bootstrap-stage1-output")
            .expect("output repo");
        assert!(output_repo
            .baseurl
            .as_deref()
            .unwrap()
            .ends_with("out/x86_64"));
    }

    #[test]
    fn a_fixed_point_profile_hides_seed_names_for_renamed_outputs() {
        let directory = tempfile::tempdir().unwrap();
        fixture(directory.path());
        let manifest = directory.path().join("bootstrap.yaml");
        let mut plan = rpmx_core::bootstrap::plan_manifest(&manifest, Some("x86_64")).unwrap();
        plan.stages[1]
            .replaces
            .insert("gcc".to_string(), vec!["gcc-seed".to_string()]);
        let stages: Vec<_> = plan.stages.iter().collect();
        let effective =
            synthesize_profile(&plan, &stages, &plan.stages[1], "x86_64", directory.path())
                .unwrap();
        let seed = effective
            .repositories
            .iter()
            .find(|repo| repo.id == "base")
            .expect("seed repo");
        assert!(
            seed.exclude.contains(&"gcc-seed".to_string()),
            "{:?}",
            seed.exclude
        );

        let output = directory.path().join("out/x86_64");
        let names = ["gcc".to_string(), "glibc".to_string()]
            .into_iter()
            .collect();
        let iteration =
            super::fixed_point_profile(&effective, &plan.stages[1], &output, &names).unwrap();
        let input = iteration
            .repositories
            .iter()
            .find(|repo| repo.id == "bootstrap-stage1-stage0")
            .expect("input repo");
        assert!(
            input.exclude.contains(&"gcc-seed".to_string()),
            "{:?}",
            input.exclude
        );
        let seed = iteration
            .repositories
            .iter()
            .find(|repo| repo.id == "base")
            .expect("seed repo");
        assert!(
            seed.exclude.contains(&"gcc-seed".to_string()),
            "{:?}",
            seed.exclude
        );
    }

    #[test]
    fn publishing_replaces_a_read_only_root() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let temp = directory.path().join("candidate");
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::write(temp.join("new"), "new").unwrap();
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o555)).unwrap();

        let destination = directory.path().join("root");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("old"), "old").unwrap();
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o555)).unwrap();

        publish(&temp, &destination, true, "stage root").unwrap();

        assert!(destination.join("new").exists());
        assert!(!destination.join("old").exists());
        let mode = std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o555);
    }

    #[test]
    fn a_non_empty_destination_needs_force() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("out");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("package.rpm"), "x").unwrap();
        let error =
            ensure_replaceable(&output, "stage output", false).expect_err("non-empty output");
        assert_eq!(error.code, codes::BOOTSTRAP_008);
        assert!(error.message.contains("--force"), "{}", error.message);
        ensure_replaceable(&output, "stage output", true).unwrap();
    }
}
