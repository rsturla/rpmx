//! Resolve build inputs without assembling or executing them.
use crate::domain::{error::codes, root::RootLock, Arch, Error, Plan, RecipeBundle};
use crate::{environment::Environment, ApplicationConfig, BootstrapEnvironment, BuildRoot};

/// Exact dependency inputs. Root directories and runner assets are execution state.
#[derive(Default)]
pub(crate) struct PreparedInputs {
    pub root: Option<RootLock>,
    pub sysroot: Option<RootLock>,
    pub cross: Option<crate::domain::guest::CrossToolchain>,
}

impl PreparedInputs {
    pub fn resolve(
        bundle: &RecipeBundle,
        plan: &Plan,
        target: Arch,
        config: &ApplicationConfig,
    ) -> Result<Self, Error> {
        let inputs = match &config.root {
            BuildRoot::UnsafeHost => Self::default(),
            BuildRoot::Locked { lock, .. } => Self {
                root: Some(crate::domain::root::load_lock(lock)?),
                ..Self::default()
            },
            BuildRoot::Bootstrap(bootstrap) => {
                let (build, sysroot, cross) = match bootstrap {
                    BootstrapEnvironment::Native { build } => (build, None, None),
                    BootstrapEnvironment::Cross {
                        build,
                        target,
                        toolchain,
                    } => (build, Some(target), Some(toolchain.as_ref().clone())),
                };
                Self {
                    root: Some(crate::domain::root::load_lock(&build.lock)?),
                    sysroot: sysroot
                        .map(|root| crate::domain::root::load_lock(&root.lock))
                        .transpose()?,
                    cross,
                }
            }
            BuildRoot::Automatic { profile } => {
                let requirement = rpmx_root::recipe::requirement(
                    &bundle.recipe,
                    plan,
                    &bundle.root,
                    profile.as_deref(),
                    target.as_str(),
                )?
                .ok_or_else(|| {
                    Error::new(
                        codes::ROOT_016,
                        "no build root: declare build.root or select a build profile",
                    )
                })?;
                let cache = config
                    .cache
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(crate::support::default_cache)?;
                Self {
                    root: Some(crate::autoroot::resolve_root(
                        &requirement.profile,
                        &requirement.name,
                        &requirement.digest,
                        &cache,
                        config.offline,
                    )?),
                    sysroot: requirement
                        .sysroot
                        .as_ref()
                        .map(|root| {
                            crate::autoroot::resolve_root(
                                &root.profile,
                                &root.name,
                                &root.digest,
                                &cache,
                                config.offline,
                            )
                        })
                        .transpose()?,
                    cross: requirement.cross,
                }
            }
        };
        if let Some(root) = &inputs.root {
            root.require_target(inputs.exec(target)?.as_str())?;
        }
        if let Some(root) = &inputs.sysroot {
            root.require_target(target.as_str())?;
        }
        Ok(inputs)
    }

    pub fn exec(&self, target: Arch) -> Result<Arch, Error> {
        if self.cross.is_some() {
            Arch::current()
        } else {
            Ok(target)
        }
    }

    pub fn materialize(
        &self,
        target: Arch,
        config: &ApplicationConfig,
    ) -> Result<Environment, Error> {
        Environment::from_inputs(self, target, config)
    }

    pub fn from_environment(environment: Option<&Environment>) -> Self {
        Self {
            root: environment.and_then(|env| env.lock().cloned()),
            sysroot: environment
                .and_then(|env| env.sysroot.as_ref().map(|root| root.lock().clone())),
            cross: environment.and_then(|env| env.cross.clone()),
        }
    }
}
