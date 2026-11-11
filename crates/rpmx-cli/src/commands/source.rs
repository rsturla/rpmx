use super::{application, SourceArgs};
use rpmx_application::{Application, ApplicationRequest, Operation, SourceInput};
use rpmx_core::Error;
use std::path::Path;

pub fn run(
    args: SourceArgs,
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
    config.runner = args.runner;
    config.offline = args.offline;
    config.proxy = args.proxy;
    config.runner_assets = args.runner_asset_root;
    config.quiet = args.quiet;
    let report = Application::new(config).execute(ApplicationRequest {
        operation: Operation::Source(SourceInput {
            recipe: args.file,
            arch: args.arch,
            locked: args.locked,
        }),
        output: args.output,
    })?;
    Ok(report)
}
