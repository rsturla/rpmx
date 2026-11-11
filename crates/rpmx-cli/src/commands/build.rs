use super::{application, BuildArgs};
use rpmx_application::{Application, ApplicationRequest, BuildInput, Operation};
use rpmx_core::Error;
use std::path::{Path, PathBuf};

pub fn run(
    args: BuildArgs,
    pipelines: Option<&Path>,
) -> Result<rpmx_application::RunReport, Error> {
    let mut config = application::config(&args.runtime, pipelines);
    config.root = application::root(
        args.policy.profile.clone(),
        args.root_lock,
        args.root,
        args.unsafe_host_root,
    )?;
    config.policy = application::policy(&args.policy, args.source_date_epoch);
    config.signing = application::signing(&args.signing);
    config.runner = args.runner;
    config.offline = args.offline;
    config.proxy = args.proxy;
    config.runner_assets = args.runner_asset_root;
    let input = BuildInput {
        recipe: args.file,
        architectures: args.arch,
        layout: rpmx_application::BuildOutputLayout::Architectures,
        arch_jobs: args.arch_jobs,
        cell: args.cell,
        stage: args.stage,
        build_tests: !args.no_tests,
        locked: args.locked,
        lint: !args.no_lint,
        index: !args.no_index,
        sbom: args.sbom,
    };
    let report = Application::new(config).execute(ApplicationRequest {
        operation: Operation::Build(input),
        output: args.output.unwrap_or_else(|| PathBuf::from("packages")),
    })?;
    Ok(report)
}
