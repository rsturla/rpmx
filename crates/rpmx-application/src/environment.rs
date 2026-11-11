//! Derive execution context once from the recipe policy. Callers choose a
//! target and a root policy, never an independent execution architecture.
use crate::domain::{
    error::codes,
    root::{RootLock, RootSelection},
    Arch, Error, Plan, RecipeBundle,
};
use crate::support::{autoroot, resolve_policy, PlanPolicy, PolicyInput};
use crate::{ApplicationConfig, BootstrapEnvironment, BuildRoot};
use std::path::{Path, PathBuf};

pub(crate) struct Environment {
    pub target: Arch,
    pub exec: Arch,
    pub selected: Option<RootSelection>,
    pub lock_path: Option<PathBuf>,
    pub sysroot: Option<RootSelection>,
    pub cross: Option<crate::domain::guest::CrossToolchain>,
    pub appliance: Option<PathBuf>,
}

impl Environment {
    pub fn prepare(
        bundle: &RecipeBundle,
        plan: &Plan,
        target: Arch,
        config: &ApplicationConfig,
    ) -> Result<Self, Error> {
        if let BuildRoot::Bootstrap(bootstrap) = &config.root {
            let (build, target_root, cross) = match bootstrap {
                BootstrapEnvironment::Native { build } => (build, None, None),
                BootstrapEnvironment::Cross {
                    build,
                    target,
                    toolchain,
                } => (build, Some(target), Some(toolchain.as_ref().clone())),
            };
            let exec = if target_root.is_some() {
                Arch::current()?
            } else {
                target
            };
            let selected = RootSelection::load(&build.lock, &build.path, exec.as_str())?;
            let sysroot = target_root
                .map(|root| RootSelection::load(&root.lock, &root.path, target.as_str()))
                .transpose()?;
            let appliance = autoroot::appliance_for_lock(
                Some(&build.lock),
                config.cache.as_deref(),
                config.offline,
                config.runner.parse()?,
                exec.as_str(),
                config.runner_assets.as_deref(),
            )?;
            return Ok(Self {
                target,
                exec,
                selected: Some(selected),
                lock_path: Some(build.lock.clone()),
                sysroot,
                cross,
                appliance,
            });
        }
        let automatic = match &config.root {
            BuildRoot::Automatic { .. } => autoroot::ensure(
                &bundle.recipe,
                plan,
                &bundle.root,
                config.root.profile(),
                config.cache.as_deref(),
                config.offline,
                false,
                target.as_str(),
            )?,
            _ => None,
        };
        if let Some(automatic) = automatic {
            let selected = automatic.selection;
            let sysroot = automatic.sysroot.map(|root| root.selection);
            let exec = if sysroot.is_some() {
                Arch::current()?
            } else {
                target
            };
            let appliance = autoroot::appliance_for_lock(
                Some(&automatic.lock),
                config.cache.as_deref(),
                config.offline,
                config.runner.parse()?,
                exec.as_str(),
                config.runner_assets.as_deref(),
            )?;
            return Ok(Self {
                target,
                exec,
                selected: Some(selected),
                lock_path: Some(automatic.lock),
                sysroot,
                cross: automatic.cross,
                appliance,
            });
        }
        Self::explicit(target, config)
    }
    pub fn explicit(target: Arch, config: &ApplicationConfig) -> Result<Self, Error> {
        let selected = match &config.root {
            BuildRoot::Locked { lock, root } => {
                let document = crate::domain::root::load_lock(lock)?;
                let path = match root {
                    Some(path) => path.clone(),
                    None => {
                        autoroot::ensure_from_lock(
                            &document,
                            config.cache.as_deref(),
                            config.offline,
                            target.as_str(),
                            false,
                            config.runner_assets.as_deref(),
                            true,
                            false,
                        )?
                        .root
                    }
                };
                Some(RootSelection::load(lock, &path, target.as_str())?)
            }
            BuildRoot::UnsafeHost => None,
            _ => {
                return Err(Error::new(
                    codes::ROOT_016,
                    "no build root: declare build.root or select a build profile",
                ))
            }
        };
        let lock_path = config.root.lock().map(Path::to_path_buf);
        let appliance = autoroot::appliance_for_lock(
            lock_path.as_deref(),
            config.cache.as_deref(),
            config.offline,
            config.runner.parse()?,
            target.as_str(),
            config.runner_assets.as_deref(),
        )?;
        Ok(Self {
            target,
            exec: target,
            selected,
            lock_path,
            sysroot: None,
            cross: None,
            appliance,
        })
    }
    pub fn root(&self) -> Option<&Path> {
        self.selected.as_ref().map(|root| root.path())
    }
    pub fn lock(&self) -> Option<&RootLock> {
        self.selected.as_ref().map(|root| root.lock())
    }
    pub fn prefix(&self) -> &str {
        self.lock()
            .map(|lock| lock.prefix.as_str())
            .unwrap_or(crate::domain::guest::PREFIX)
    }
    pub fn libdir(&self) -> &str {
        self.lock()
            .map(|lock| lock.libdir.as_str())
            .unwrap_or(crate::domain::guest::LIBDIR)
    }
}

pub(crate) fn target(
    recipe: &Path,
    explicit: Option<&str>,
    config: &ApplicationConfig,
) -> Result<Arch, Error> {
    if let Some(arch) = explicit {
        return Arch::parse(arch);
    }
    let bundle = crate::domain::load_recipe_bundle(recipe)?;
    let declared = autoroot::declared_arch(&bundle.recipe, &bundle.root, config.root.profile())?;
    declared
        .as_deref()
        .map(Arch::parse)
        .unwrap_or_else(Arch::current)
}

pub(crate) fn policy(config: &ApplicationConfig) -> Result<crate::policy::ResolvedPolicy, Error> {
    resolve_policy(PolicyInput {
        profile: config.root.profile(),
        dist_tag: config.policy.dist_tag.as_deref(),
        build_release: config.policy.build_release,
    })
}

pub(crate) fn plan(
    recipe: &Path,
    target: Arch,
    config: &ApplicationConfig,
    policy: PlanPolicy<'_>,
) -> Result<(RecipeBundle, Plan), Error> {
    rpmx_root::recipe::load_plan(
        recipe,
        target.as_str(),
        config.pipelines.as_deref(),
        false,
        true,
        policy,
    )
}
