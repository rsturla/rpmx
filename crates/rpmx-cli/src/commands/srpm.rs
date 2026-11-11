use super::{application, SrpmArgs};
use rpmx_application::{Application, ApplicationRequest, Operation, SrpmInput};
use rpmx_core::Error;
use std::path::Path;

pub fn run(args: SrpmArgs, pipelines: Option<&Path>) -> Result<rpmx_application::RunReport, Error> {
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
    config.quiet = args.quiet;
    let report = Application::new(config).execute(ApplicationRequest {
        operation: Operation::Srpm(SrpmInput {
            recipe: args.file,
            arch: args.arch,
            sources: args.sources,
            cell: args.cell,
        }),
        output: args.output,
    })?;
    Ok(report)
}
