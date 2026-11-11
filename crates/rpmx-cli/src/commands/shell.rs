use super::{application, ShellArgs};
use rpmx_application::{Application, ApplicationConfig, ApplicationRequest, DebugInput, Operation};
use rpmx_core::Error;
use std::path::Path;

pub fn run(
    args: ShellArgs,
    pipelines: Option<&Path>,
) -> Result<rpmx_application::RunReport, Error> {
    let config = ApplicationConfig {
        cache: args.cache.cache,
        offline: args.offline,
        pipelines: pipelines.map(Path::to_path_buf),
        root: application::root(
            args.policy.profile.clone(),
            args.root_lock,
            args.root,
            false,
        )?,
        policy: application::policy(&args.policy, None),
        ..ApplicationConfig::default()
    };
    // Debug reports live outside the mounted workspace and never consume its
    // stdout; inherited streams remain owned by the interactive command.
    let output = tempfile::tempdir()
        .map_err(|e| Error::new(rpmx_core::error::codes::BUILD_005, e.to_string()))?;
    let report = Application::new(config).execute(ApplicationRequest {
        operation: Operation::Debug(DebugInput {
            recipe: args.file,
            arch: args.arch,
            workspace: args.workspace,
            stage: args.stage,
            command: args.command,
        }),
        output: output.path().into(),
    })?;
    Ok(report)
}
