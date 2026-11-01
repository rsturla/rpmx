use crate::cli::{self, Cli, Command};
use crate::commands;
use rpmx_core::Error;

fn resolve_policy(policy: cli::PolicyArgs) -> Result<commands::ResolvedPolicy, Error> {
    commands::resolve_policy(commands::PolicyInput {
        profile: policy.profile.as_deref(),
        dist_tag: policy.dist_tag.as_deref(),
        build_release: policy.build_release,
    })
}

pub fn run(cli: Cli) -> Result<(), Error> {
    let Cli {
        pipeline_dir,
        command,
    } = cli;
    match command {
        Command::Lint {
            file,
            arch,
            policy,
            json,
        } => {
            let resolved = resolve_policy(policy)?;
            commands::lint::run(
                &file,
                &arch,
                pipeline_dir.as_deref(),
                resolved.plan_policy(),
                json,
            )
        }
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
        Command::Fetch {
            file,
            cache,
            proxy,
            offline,
            json,
        } => commands::fetch::run(commands::FetchArgs {
            file: &file,
            cache: cache.as_deref(),
            proxy: proxy.as_deref(),
            offline,
            json,
        }),
        Command::Plan {
            file,
            arch,
            policy,
            json,
        } => {
            let resolved = resolve_policy(policy)?;
            commands::plan::run(
                &file,
                &arch,
                pipeline_dir.as_deref(),
                resolved.plan_policy(),
                json,
            )
        }
        Command::Source {
            file,
            arch,
            runner,
            policy,
            cache,
            proxy,
            offline,
            root_lock,
            root,
            unsafe_host_root,
            output,
            source_date_epoch,
            jobs,
            step_timeout,
            json,
        } => {
            let resolved = resolve_policy(policy)?;
            commands::source::run(commands::SourceArgs {
                file: &file,
                pipeline_dir: pipeline_dir.as_deref(),
                policy: resolved.plan_policy(),
                cache: cache.as_deref(),
                proxy: proxy.as_deref(),
                offline,
                output: &output,
                arch: &arch,
                runner: &runner,
                source_date_epoch,
                jobs,
                step_timeout,
                root_lock: root_lock.as_deref(),
                root: root.as_deref(),
                unsafe_host_root,
                quiet: false,
                json,
            })
            .map(|_| ())
        }
        Command::Srpm {
            file,
            policy,
            sources,
            output,
            arch,
            source_date_epoch,
            cell,
            root_lock,
            signing_key,
            signing_passphrase_env,
            signing_timestamp,
            json,
        } => {
            let resolved = resolve_policy(policy)?;
            commands::srpm::run(commands::SrpmArgs {
                file: &file,
                sources: &sources,
                output: &output,
                arch: &arch,
                source_date_epoch,
                cell: cell.as_deref(),
                pipeline_dir: pipeline_dir.as_deref(),
                policy: resolved.plan_policy(),
                root_lock: root_lock.as_deref(),
                signing_key: signing_key.as_deref(),
                signing_passphrase_env: signing_passphrase_env.as_deref(),
                signing_timestamp,
                quiet: false,
                json,
            })
            .map(|_| ())
        }
        Command::Rebuild {
            file,
            policy,
            output,
            arch,
            runner,
            cell,
            jobs,
            step_timeout,
            root_image,
            root_lock,
            root,
            run_tests,
            sbom,
            test_root_lock,
            signing_key,
            signing_passphrase_env,
            signing_timestamp,
            json,
        } => {
            let resolved = resolve_policy(policy)?;
            commands::rebuild::run(commands::RebuildArgs {
                file: &file,
                output: &output,
                arch: &arch,
                runner: &runner,
                cell: cell.as_deref(),
                policy: resolved.plan_policy(),
                jobs,
                step_timeout,
                root_image: root_image.as_deref(),
                root_lock: root_lock.as_deref(),
                root: root.as_deref(),
                run_tests,
                sbom: sbom.as_deref(),
                test_root_lock: test_root_lock.as_deref(),
                signing_key: signing_key.as_deref(),
                signing_passphrase_env: signing_passphrase_env.as_deref(),
                signing_timestamp,
                quiet: false,
                json,
            })
            .map(|_| ())
        }
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
