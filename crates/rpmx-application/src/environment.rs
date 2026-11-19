//! Derive execution context once from the recipe policy. Callers choose a
//! target and a root policy, never an independent execution architecture.
use crate::domain::{
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
        crate::inputs::PreparedInputs::for_source(bundle, plan, target, config)?
            .materialize(target, config)
    }

    pub fn explicit(target: Arch, config: &ApplicationConfig) -> Result<Self, Error> {
        let inputs = match &config.root {
            BuildRoot::Locked { lock, .. } => crate::inputs::PreparedInputs {
                root: Some(crate::domain::root::load_lock(lock)?),
                ..Default::default()
            },
            BuildRoot::UnsafeHost => crate::inputs::PreparedInputs::default(),
            _ => {
                return Err(Error::new(
                    crate::domain::error::codes::ROOT_016,
                    "no build root: declare build.root or select a build profile",
                ))
            }
        };
        inputs.materialize(target, config)
    }

    /// Assemble and verify exact inputs. This operation never solves packages.
    pub fn from_inputs(
        inputs: &crate::inputs::PreparedInputs,
        target: Arch,
        config: &ApplicationConfig,
        allow_assets: bool,
    ) -> Result<Self, Error> {
        let exec = inputs.exec(target)?;
        let (build_path, sysroot_path, supplied_lock) = match &config.root {
            BuildRoot::Locked { lock, root } => (root.as_deref(), None, Some(lock.as_path())),
            BuildRoot::Bootstrap(BootstrapEnvironment::Native { build }) => {
                (Some(build.path.as_path()), None, Some(build.lock.as_path()))
            }
            BuildRoot::Bootstrap(BootstrapEnvironment::Cross { build, target, .. }) => (
                Some(build.path.as_path()),
                Some(target.path.as_path()),
                Some(build.lock.as_path()),
            ),
            _ => (None, None, None),
        };
        let mut appliance = None;
        let selected = inputs
            .root
            .as_ref()
            .map(|lock| {
                let path = match build_path {
                    Some(path) => path.to_path_buf(),
                    None => {
                        let roots = autoroot::ensure_from_lock(
                            lock,
                            config.cache.as_deref(),
                            config.offline,
                            exec.as_str(),
                            false,
                            config.runner_assets.as_deref(),
                            allow_assets,
                            inputs.cross.is_some(),
                        )?;
                        appliance = roots.appliance;
                        roots.root
                    }
                };
                if let Some(supplied) = supplied_lock {
                    let explicit = crate::domain::root::load_lock(supplied)?;
                    if explicit.assembly_identity() != lock.assembly_identity() {
                        return Err(Error::new(
                            crate::domain::error::codes::ROOT_016,
                            "explicit root lock differs from the prepared build inputs",
                        ));
                    }
                    RootSelection::verify_inputs(&explicit, &path, exec.as_str())
                } else {
                    RootSelection::verify_inputs(lock, &path, exec.as_str())
                }
            })
            .transpose()?;
        let sysroot = inputs
            .sysroot
            .as_ref()
            .map(|lock| {
                let path = match sysroot_path {
                    Some(path) => path.to_path_buf(),
                    None => autoroot::ensure_sysroot_from_lock(
                        lock,
                        config.cache.as_deref(),
                        config.offline,
                        target.as_str(),
                    )?,
                };
                RootSelection::verify_inputs(lock, &path, target.as_str())
            })
            .transpose()?;
        let lock_path = supplied_lock.map(Path::to_path_buf).or_else(|| {
            selected.as_ref().map(|root| {
                root.path()
                    .parent()
                    .expect("cached root parent")
                    .join("root.lock.yaml")
            })
        });
        if appliance.is_none() {
            appliance = selected
                .as_ref()
                .map(|root| {
                    autoroot::appliance_for(
                        root.lock(),
                        config.cache.as_deref(),
                        config.offline,
                        config.runner.parse()?,
                        exec.as_str(),
                        config.runner_assets.as_deref(),
                        allow_assets,
                    )
                })
                .transpose()?
                .flatten();
        }
        Ok(Self {
            target,
            exec,
            selected,
            lock_path,
            sysroot,
            cross: inputs.cross.clone(),
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
