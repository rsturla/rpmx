use rpmx_core::Error;

use crate::cli::BootstrapCommand;

mod build;
mod plan;
mod rebuild;

pub fn run(command: BootstrapCommand) -> Result<(), Error> {
    match command {
        BootstrapCommand::Plan(args) => plan::run(args),
        BootstrapCommand::Build(args) => build::run(args),
        BootstrapCommand::Rebuild(args) => rebuild::run(*args),
    }
}
