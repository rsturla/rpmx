use crate::cli::{self, Cli, Command};
use crate::commands;
use rpmx_core::Error;

pub enum CommandOutcome {
    Done,
    Application {
        report: Box<rpmx_application::RunReport>,
        quiet: bool,
    },
}

pub fn run(cli: Cli) -> Result<CommandOutcome, Error> {
    let Cli {
        pipeline_dir,
        keep_workdir: _,
        debug: _,
        json: _,
        verbose: _,
        color: _,
        command,
    } = cli;
    let result = match command {
        Command::Explain(args) => commands::explain::run(args, pipeline_dir.as_deref()),
        Command::Shell(args) => {
            return commands::shell::run(args, pipeline_dir.as_deref()).map(|report| {
                CommandOutcome::Application {
                    report: Box::new(report),
                    quiet: true,
                }
            })
        }
        Command::Steps { command } => match command {
            cli::StepsCommand::List(args) => commands::steps::list(args),
            cli::StepsCommand::Show(args) => commands::steps::show(args),
        },
        Command::Doctor(args) => commands::doctor::run(args),
        Command::Bump(args) => commands::bump::run(args),
        Command::Init(args) => commands::init::run(args),
        Command::Lint(args) => commands::lint::run(args, pipeline_dir.as_deref()),
        Command::Lock(args) => commands::lock::run(args, pipeline_dir.as_deref()),
        Command::Plan(args) => commands::plan::run(args, pipeline_dir.as_deref()),
        Command::Bootstrap { command } => commands::bootstrap::run(command),
        Command::Root { command } => match command {
            cli::RootCommand::Resolve(args) => commands::root::resolve(args),
            cli::RootCommand::Assemble(args) => commands::root::assemble(args),
            cli::RootCommand::Digest(args) => commands::root::digest(args),
            cli::RootCommand::Export(args) => commands::root::export(args),
            cli::RootCommand::Import(args) => commands::root::import(args),
        },
        Command::Runner { command } => match command {
            cli::RunnerCommand::Resolve(args) => commands::appliance::resolve(args),
            cli::RunnerCommand::Assemble(args) => commands::appliance::assemble(args),
            cli::RunnerCommand::Inspect(args) => commands::appliance::inspect(args),
        },
        Command::Fetch(args) => commands::fetch::run(args),
        Command::Source(args) => {
            let quiet = args.quiet;
            return commands::source::run(args, pipeline_dir.as_deref()).map(|report| {
                CommandOutcome::Application {
                    report: Box::new(report),
                    quiet,
                }
            });
        }
        Command::Srpm(args) => {
            let quiet = args.quiet;
            return commands::srpm::run(args, pipeline_dir.as_deref()).map(|report| {
                CommandOutcome::Application {
                    report: Box::new(report),
                    quiet,
                }
            });
        }
        Command::Rebuild(args) => {
            let quiet = args.quiet;
            return commands::rebuild::run(args).map(|report| CommandOutcome::Application {
                report: Box::new(report),
                quiet,
            });
        }
        Command::Build(args) => {
            return commands::build::run(args, pipeline_dir.as_deref()).map(|report| {
                CommandOutcome::Application {
                    report: Box::new(report),
                    quiet: false,
                }
            })
        }
        Command::Test(args) => {
            return commands::testcmd::run(args, pipeline_dir.as_deref()).map(|report| {
                CommandOutcome::Application {
                    report: Box::new(report),
                    quiet: false,
                }
            })
        }
        Command::Repo(args) => commands::repo::run(args),
        Command::Cache { command } => match command {
            cli::CacheCommand::Gc(args) => commands::cache::gc(args),
        },
        Command::Keygen(args) => commands::sign::keygen(args),
        Command::Sign(args) => commands::sign::run(args),
    };
    result.map(|()| CommandOutcome::Done)
}
