//! Application operations own environment selection, execution and evidence.
//! CLI and automation callers supply domain inputs rather than runner details.
mod domain;
mod support;
use rpmx_root::recipe::policy;
mod autoroot;
mod emit;
use rpmx_build::package_lint;
use rpmx_build::package_lock;
mod acceptance;
mod debug;
mod environment;
mod production;
mod report;
mod request;

pub use acceptance::{AcceptInput, AcceptanceEnvironment, RpmInputs};
pub use report::*;
pub use request::*;

use rpmx_base::Error;

pub struct Application {
    config: ApplicationConfig,
}
impl Application {
    pub fn new(config: ApplicationConfig) -> Self {
        Self { config }
    }

    /// Preparation errors are ordinary failed runs. Only failure to establish
    /// a writable report destination returns Err without a persisted report.
    pub fn execute(&self, request: ApplicationRequest) -> Result<RunReport, Error> {
        std::fs::create_dir_all(&request.output).map_err(|e| {
            Error::new(
                rpmx_base::error::codes::BUILD_001,
                format!(
                    "cannot create report directory {}: {e}",
                    request.output.display()
                ),
            )
        })?;
        let operation = match &request.operation {
            Operation::Source(_) => "source",
            Operation::Srpm(_) => "srpm",
            Operation::Build(_) => "build",
            Operation::Rebuild(_) => "rebuild",
            Operation::Accept(_) => "accept",
            Operation::Debug(_) => "debug",
        };
        let mut report = RunReport::new(operation);
        report.write(&request.output)?;
        let result = match &request.operation {
            Operation::Source(input) => {
                production::source::execute(&self.config, input, &request.output, &mut report)
            }
            Operation::Srpm(input) => {
                production::srpm::execute(&self.config, input, &request.output, &mut report)
            }
            Operation::Build(input) => {
                production::build::execute(&self.config, input, &request.output, &mut report)
            }
            Operation::Rebuild(input) => {
                production::rebuild::execute(&self.config, input, &request.output, &mut report)
            }
            Operation::Accept(input) => {
                acceptance::execute(&self.config, input, &request.output, &mut report)
            }
            Operation::Debug(input) => debug::execute(&self.config, input),
        };
        if let Err(error) = result {
            report.fail(&error);
        }
        report.complete();
        report.write(&request.output)?;
        Ok(report)
    }
}
