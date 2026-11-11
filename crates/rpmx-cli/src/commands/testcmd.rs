use super::{application, TestArgs};
use rpmx_application::{
    AcceptInput, AcceptanceEnvironment, Application, ApplicationRequest, Operation, RpmInputs,
};
use rpmx_core::Error;
use std::path::Path;

pub fn run(args: TestArgs, pipelines: Option<&Path>) -> Result<rpmx_application::RunReport, Error> {
    let mut config = application::config(&args.runtime, pipelines);
    config.runner = args.runner;
    config.offline = args.offline;
    config.runner_assets = args.runner_asset_root;
    config.policy.dist_tag = args.dist_tag;
    config.policy.build_release = args.build_release;
    let environment = if let Some(path) = args.replay {
        AcceptanceEnvironment::Replay(path)
    } else if let Some(path) = args.test_profile {
        AcceptanceEnvironment::Profile(path)
    } else {
        AcceptanceEnvironment::ProjectDefault
    };
    let input = AcceptInput {
        recipe: args.file,
        subjects: RpmInputs::Directory(args.install_dir),
        arch: args.arch,
        selection: args.selected,
        environment,
        save_environment: args.save_environment,
    };
    let mut report = Application::new(config).execute(ApplicationRequest {
        operation: Operation::Accept(input),
        output: args.test_output.clone(),
    })?;
    if let Some(path) = args.junit {
        if let Err(error) = report.write_junit(&path) {
            report.fail(&error);
            if let Err(error) = report.write(&args.test_output) {
                report.fail(&error);
            }
        }
    }
    Ok(report)
}
