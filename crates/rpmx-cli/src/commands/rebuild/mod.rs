use crate::commands::{application, RebuildArgs};
use rpmx_application::{Application, ApplicationRequest, Operation, RebuildInput};
use rpmx_core::Error;
use std::path::Path;

pub fn run(args: RebuildArgs) -> Result<rpmx_application::RunReport, Error> {
    run_with(args, false, None)
}

pub fn run_with(
    args: RebuildArgs,
    replace_root_lock: bool,
    srpm_out: Option<&Path>,
) -> Result<rpmx_application::RunReport, Error> {
    let mut config = application::config(&args.runtime, None);
    config.root = application::root(
        args.policy.profile.clone(),
        args.root_lock,
        args.root,
        false,
    )?;
    config.policy = application::policy(&args.policy, None);
    config.signing = application::signing(&args.signing);
    config.runner = args.runner;
    config.offline = args.offline;
    config.runner_assets = args.runner_asset_root;
    config.quiet = args.quiet;
    let arch = args
        .arch
        .clone()
        .map(Ok)
        .unwrap_or_else(|| rpmx_rpm::package_arch(&args.file))?;
    let output = args
        .output
        .unwrap_or_else(|| crate::commands::default_output(&arch));
    let input = RebuildInput {
        srpm: args.file,
        arch: args.arch,
        cell: args.cell,
        build_tests: !args.no_tests,
        lint: !args.no_lint,
        index: !args.no_index,
        sbom: args.sbom,
        replace_root_lock,
        replacement_srpm: srpm_out.map(Path::to_path_buf),
    };
    let report = Application::new(config).execute(ApplicationRequest {
        operation: Operation::Rebuild(input),
        output,
    })?;
    Ok(report)
}
