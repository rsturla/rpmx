use super::{application, InputsArgs};
use rpmx_application::{
    Application, ApplicationConfig, ApplicationRequest, BuildRoot, Operation, ResolveInputsInput,
};
use std::path::Path;

pub fn run(
    args: InputsArgs,
    pipelines: Option<&Path>,
) -> Result<rpmx_application::RunReport, rpmx_core::Error> {
    Application::new(ApplicationConfig {
        root: BuildRoot::Automatic {
            profile: args.policy.profile.clone(),
        },
        policy: application::policy(&args.policy, None),
        pipelines: pipelines.map(Path::to_path_buf),
        cache: args.cache,
        offline: args.offline,
        ..Default::default()
    })
    .execute(ApplicationRequest {
        operation: Operation::ResolveInputs(ResolveInputsInput {
            recipe: args.file,
            arch: args.arch,
            include_optional_tests: args.include_optional_tests,
        }),
        output: args.output,
    })
}
