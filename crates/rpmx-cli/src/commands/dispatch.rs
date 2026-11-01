use crate::cli::{self, Cli, Command};
use crate::commands;
use rpmx_core::Error;

pub fn run(cli: Cli) -> Result<(), Error> {
    let Cli {
        pipeline_dir,
        command,
    } = cli;
    match command {
        Command::Lint(args) => commands::lint::run(args, pipeline_dir.as_deref()),
        Command::Plan(args) => commands::plan::run(args, pipeline_dir.as_deref()),
        Command::Root { command } => match command {
            cli::RootCommand::Resolve {
                profile,
                output,
                work,
                keep,
                cache,
                json,
            } => commands::root::resolve(commands::RootResolveArgs {
                profile: &profile,
                output: output.as_deref(),
                work: work.as_deref(),
                keep,
                cache: cache.as_deref(),
                json,
            }),
            cli::RootCommand::Assemble {
                lock,
                root,
                output,
                cache,
                work,
                keep,
                json,
            } => commands::root::assemble(commands::RootAssembleArgs {
                lock: &lock,
                root: root.as_deref(),
                output: output.as_deref(),
                cache: cache.as_deref(),
                work: work.as_deref(),
                keep,
                json,
            }),
            cli::RootCommand::Digest { root, json } => {
                commands::root::digest(commands::RootDigestArgs { root: &root, json })
            }
            cli::RootCommand::Export { lock, output } => {
                commands::root::export(commands::RootExportArgs {
                    lock: &lock,
                    output: output.as_deref(),
                })
            }
            cli::RootCommand::Import {
                manifest,
                profile,
                arch,
                releasever,
                output,
            } => commands::root::import(commands::RootImportArgs {
                manifest: &manifest,
                profile: &profile,
                arch: arch.as_deref(),
                releasever: &releasever,
                output: output.as_deref(),
            }),
        },
        Command::Fetch(args) => commands::fetch::run(args),
        Command::Source(args) => commands::source::run(args, pipeline_dir.as_deref()).map(|_| ()),
        Command::Srpm(args) => commands::srpm::run(args, pipeline_dir.as_deref()).map(|_| ()),
        Command::Rebuild(args) => commands::rebuild::run(args).map(|_| ()),
        Command::Build(args) => commands::build::run(args, pipeline_dir.as_deref()).map(|_| ()),
        Command::Repo {
            rpms,
            output,
            signing_key,
            signing_passphrase_env,
            signing_timestamp,
            json,
        } => commands::repo::run(commands::repo::RepoArgs {
            rpms: &rpms,
            output: &output,
            signing_key: signing_key.as_deref(),
            signing_passphrase_env: signing_passphrase_env.as_deref(),
            signing_timestamp,
            json,
        }),
        Command::Cache { command } => match command {
            cli::CacheCommand::Gc {
                lock,
                recipe,
                older_than_hours,
                dry_run,
                cache,
                json,
            } => commands::cache::gc(commands::cache::GcArgs {
                locks: &lock,
                recipes: &recipe,
                older_than_hours,
                dry_run,
                cache: cache.as_deref(),
                json,
            }),
        },
        Command::Keygen {
            output,
            user_id,
            bits,
            passphrase_env,
            json,
        } => commands::sign::keygen(commands::sign::KeygenArgs {
            output: &output,
            user_id: &user_id,
            bits,
            passphrase_env: passphrase_env.as_deref(),
            json,
        }),
        Command::Sign {
            signing_key,
            passphrase_env,
            timestamp,
            rpms,
            json,
        } => commands::sign::run(commands::sign::SignArgs {
            signing_key: &signing_key,
            passphrase_env: passphrase_env.as_deref(),
            timestamp,
            rpms: &rpms,
            json,
        }),
    }
}
