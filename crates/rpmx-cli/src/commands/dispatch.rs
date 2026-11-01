use crate::cli::{self, Cli, Command};
use crate::commands;
use rpmx_core::Error;

pub fn run(cli: Cli) -> Result<(), Error> {
    let Cli {
        pipeline_dir,
        keep_workdir: _,
        debug: _,
        command,
    } = cli;
    match command {
        Command::Explain(args) => commands::explain::run(args, pipeline_dir.as_deref()),
        Command::Steps { command } => match command {
            cli::StepsCommand::List(args) => commands::steps::list(args),
            cli::StepsCommand::Show(args) => commands::steps::show(args),
        },
        Command::Doctor(args) => commands::doctor::run(args),
        Command::Init(args) => commands::init::run(args),
        Command::Lint(args) => commands::lint::run(args, pipeline_dir.as_deref()),
        Command::Lock(args) => commands::lock::run(args, pipeline_dir.as_deref()),
        Command::Plan(args) => commands::plan::run(args, pipeline_dir.as_deref()),
        Command::Root { command } => match command {
            cli::RootCommand::Resolve(args) => commands::root::resolve(args),
            cli::RootCommand::Assemble(args) => commands::root::assemble(args),
            cli::RootCommand::Digest(args) => commands::root::digest(args),
            cli::RootCommand::Export(args) => commands::root::export(args),
            cli::RootCommand::Import(args) => commands::root::import(args),
        },
        Command::Fetch(args) => commands::fetch::run(args),
        Command::Source(args) => commands::source::run(args, pipeline_dir.as_deref()).map(|_| ()),
        Command::Srpm(args) => commands::srpm::run(args, pipeline_dir.as_deref()).map(|_| ()),
        Command::Rebuild(args) => commands::rebuild::run(args).map(|_| ()),
        Command::Build(args) => commands::build::run(args, pipeline_dir.as_deref()).map(|_| ()),
        Command::Test(args) => commands::testcmd::run(args, pipeline_dir.as_deref()),
        Command::Repo(args) => commands::repo::run(args),
        Command::Cache { command } => match command {
            cli::CacheCommand::Gc(args) => commands::cache::gc(args),
        },
        Command::Keygen(args) => commands::sign::keygen(args),
        Command::Sign(args) => commands::sign::run(args),
    }
}
