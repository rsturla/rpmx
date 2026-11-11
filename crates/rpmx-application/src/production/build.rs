use crate::domain::{error::codes, Arch, Error};
use crate::{
    environment::{self, Environment},
    support,
};
use crate::{ApplicationConfig, BuildInput, BuildRoot, RebuildInput, RunReport};
use std::path::Path;

pub(crate) fn execute(
    config: &ApplicationConfig,
    input: &BuildInput,
    output: &Path,
    report: &mut RunReport,
) -> Result<(), Error> {
    let arches = architectures(config, input)?;
    if matches!(input.layout, crate::BuildOutputLayout::Flat) && arches.len() != 1 {
        return Err(Error::new(
            codes::BUILD_003,
            "flat build output requires one architecture",
        ));
    }
    if arches.len() > 1
        && (input.stage.is_some()
            || config.root.lock().is_some()
            || input.locked
            || input.sbom.is_some())
    {
        return Err(Error::new(codes::BUILD_003, "staged packaging, explicit locks, --locked and explicit SBOM paths require one architecture"));
    }
    report.build_tests_requested = input.stage.is_none() && input.build_tests;
    let concurrency =
        architecture_jobs(input.arch_jobs.as_deref(), arches.len(), config.jobs.get())?;
    // Workers own separate reports and architecture directories. Merge in request
    // order so concurrent completion never changes the primary diagnostic.
    for batch in arches.chunks(concurrency) {
        let results = std::thread::scope(|scope| {
            let workers: Vec<_> = batch
                .iter()
                .map(|&arch| {
                    let mut config = config.clone();
                    config.jobs =
                        std::num::NonZeroU32::new((config.jobs.get() / concurrency as u32).max(1))
                            .unwrap();
                    let mut child = RunReport::new("build");
                    child.id = report.id.clone();
                    child.build_tests_requested = input.stage.is_none() && input.build_tests;
                    let arch_output = match input.layout {
                        crate::BuildOutputLayout::Architectures => output.join(arch.as_str()),
                        crate::BuildOutputLayout::Flat => output.into(),
                    };
                    scope.spawn(move || {
                        crate::domain::output::set_arch(Some(arch.as_str()));
                        if let Err(error) = one(&config, input, &arch_output, arch, &mut child) {
                            child.fail(&error);
                        }
                        crate::domain::output::set_arch(None);
                        child
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join())
                .collect::<Vec<_>>()
        });
        for result in results {
            match result {
                Ok(mut child) => {
                    report.artifacts.rpms.append(&mut child.artifacts.rpms);
                    report.artifacts.srpms.append(&mut child.artifacts.srpms);
                    report
                        .artifacts
                        .sources
                        .append(&mut child.artifacts.sources);
                    report
                        .artifacts
                        .repository
                        .append(&mut child.artifacts.repository);
                    report.evidence.append(&mut child.evidence);
                    report.build_tests.append(&mut child.build_tests);
                    report.environments.append(&mut child.environments);
                    report.inputs.append(&mut child.inputs);
                    if let Some(error) = child.primary {
                        if report.primary.is_none() {
                            report.primary = Some(error);
                        } else {
                            report.secondary.push(error);
                        }
                    }
                    report.secondary.append(&mut child.secondary);
                }
                Err(_) => report.fail(&Error::new(
                    codes::BUILD_006,
                    "architecture build worker panicked",
                )),
            }
        }
    }
    Ok(())
}

fn architectures(config: &ApplicationConfig, input: &BuildInput) -> Result<Vec<Arch>, Error> {
    if input.architectures.is_empty() {
        return Ok(vec![environment::target(&input.recipe, None, config)?]);
    }
    let mut arches = Vec::new();
    for value in &input.architectures {
        if value == "all" {
            let bundle = crate::domain::load_recipe_bundle(&input.recipe)?;
            let declared = rpmx_root::recipe::declared_arches(
                &bundle.recipe,
                &bundle.root,
                config.root.profile(),
            )?
            .ok_or_else(|| {
                Error::new(
                    codes::CLI_001,
                    "--arch all requires a selected profile declaring its architectures",
                )
            })?;
            for value in declared {
                let arch = Arch::parse(&value)?;
                if !arches.contains(&arch) {
                    arches.push(arch);
                }
            }
        } else {
            let arch = Arch::parse(value)?;
            if !arches.contains(&arch) {
                arches.push(arch);
            }
        }
    }
    Ok(arches)
}

fn one(
    config: &ApplicationConfig,
    input: &BuildInput,
    output: &Path,
    target: Arch,
    report: &mut RunReport,
) -> Result<(), Error> {
    let policy = environment::policy(config)?;
    let (bundle, mut plan) =
        environment::plan(&input.recipe, target, config, policy.plan_policy())?;
    if let Some(id) = &input.cell {
        plan.cells.retain(|cell| cell.id == *id);
    }
    if plan.cells.is_empty() {
        return Err(Error::new(codes::BUILD_002, "no selected cells"));
    }
    // Reject recipe drift before acquiring or assembling any environment.
    // The second check binds the prepared build and target roots as well.
    if input.locked {
        support::lock::verify(
            &bundle,
            &plan,
            policy.plan_policy(),
            config.root.profile(),
            target.as_str(),
            config.policy.source_date_epoch,
            None,
            None,
            false,
            config.cache.as_deref(),
        )?;
    }
    if let Some(stage) = &input.stage {
        return super::package_stage(config, input, &bundle, &plan, target, stage, output, report);
    }
    let environment = Environment::prepare(&bundle, &plan, target, config)?;
    if input.locked {
        support::lock::verify(
            &bundle,
            &plan,
            policy.plan_policy(),
            config.root.profile(),
            target.as_str(),
            config.policy.source_date_epoch,
            environment.lock(),
            environment.sysroot.as_ref().map(|root| root.lock()),
            true,
            config.cache.as_deref(),
        )?;
    }
    let sources = output.join("source");
    let epoch = super::source::assemble_sources(
        config,
        &bundle,
        &plan,
        &environment,
        &sources,
        output,
        report,
    )?;
    // The same verified environment binds source assembly and each embedded
    // rebuild. The SRPM carries the target sysroot and cross toolchain itself.
    let mut rebuild_config = config.clone();
    if let Some(lock) = &environment.lock_path {
        rebuild_config.root = BuildRoot::Locked {
            lock: lock.clone(),
            root: environment.root().map(Path::to_path_buf),
        };
    }
    for cell in &plan.cells {
        let mut single = plan.clone();
        single.cells = vec![cell.clone()];
        let srpms = super::srpm::package(
            config,
            &bundle,
            &single,
            &super::srpm::SourceBuildProvenance::from_environment(Some(&environment)),
            &sources,
            &output.join("src"),
            epoch,
            report,
        )?;
        for srpm in srpms {
            let rebuild = RebuildInput {
                srpm,
                arch: Some(target.to_string()),
                cell: Some(cell.id.clone()),
                build_tests: input.build_tests,
                lint: input.lint,
                index: false,
                sbom: input.sbom.clone(),
                replace_root_lock: false,
                replacement_srpm: None,
            };
            super::rebuild::execute_prepared(
                &rebuild_config,
                &rebuild,
                output,
                &environment,
                report,
            )?;
        }
    }
    if input.index {
        super::index(config, output, epoch, report)?;
    }
    Ok(())
}

fn architecture_jobs(value: Option<&str>, architectures: usize, jobs: u32) -> Result<usize, Error> {
    let count = match value {
        Some("all") => architectures,
        Some(value) => value
            .parse::<std::num::NonZeroUsize>()
            .map_err(|_| {
                Error::new(
                    codes::CLI_001,
                    "--arch-jobs requires a positive integer or all",
                )
            })?
            .get(),
        None => (std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            / jobs as usize)
            .max(1),
    };
    Ok(count.min(architectures).max(1))
}
