use crate::domain::{error::codes, sandbox::SandboxConfig, Arch, Error};
use crate::{
    environment::{self, Environment},
    support::WorkDir,
};
use crate::{ApplicationConfig, DebugInput};

pub(crate) fn execute(config: &ApplicationConfig, input: &DebugInput) -> Result<(), Error> {
    let env = if let Some(lock) = config.root.lock() {
        let document = crate::domain::root::load_lock(lock)?;
        let target = input.arch.as_deref().map(Arch::parse).unwrap_or_else(|| {
            document
                .archs
                .first()
                .map(|arch| Arch::parse(arch))
                .unwrap_or_else(|| {
                    Err(Error::new(
                        codes::ROOT_016,
                        "debug root has no target architecture",
                    ))
                })
        })?;
        Environment::explicit(target, config)?
    } else {
        let target = environment::target(&input.recipe, input.arch.as_deref(), config)?;
        let policy = environment::policy(config)?;
        let (bundle, plan) =
            environment::plan(&input.recipe, target, config, policy.plan_policy())?;
        Environment::prepare(&bundle, &plan, target, config)?
    };
    let target = env.target;
    let workspace = input
        .workspace
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .map_err(|e| {
            Error::new(
                codes::ROOT_016,
                format!("cannot choose debug workspace: {e}"),
            )
        })?;
    let work = WorkDir::new("debug")?;
    let stage = input
        .stage
        .clone()
        .unwrap_or_else(|| work.path().join("stage"));
    std::fs::create_dir_all(&stage).map_err(|e| {
        Error::new(
            codes::ROOT_016,
            format!("cannot create {}: {e}", stage.display()),
        )
    })?;
    let sandbox = SandboxConfig {
        writable_root: false,
        runner: crate::domain::runner::RunnerKind::Bwrap,
        host: Arch::current()?,
        exec: env.exec,
        target,
        sysroot: env.sysroot.as_ref().map(|root| root.path()),
        cross: env.cross.as_ref(),
        workspace: &workspace,
        stage: &stage,
        root: env.root(),
        assets: crate::domain::runner::RunnerAssets::new(env.appliance.as_deref(), &[]),
        image: None,
        prefix: env.prefix(),
        libdir: env.libdir(),
        source_date_epoch: config.policy.source_date_epoch.unwrap_or(0),
        jobs: config.jobs,
        timeout: config.timeout,
    };
    let args = input
        .command
        .as_ref()
        .map(|command| vec!["-c".into(), command.clone()])
        .unwrap_or_else(|| vec!["-i".into()]);
    crate::domain::sandbox::run_native_terminal(&sandbox, "sh", &args)
}
