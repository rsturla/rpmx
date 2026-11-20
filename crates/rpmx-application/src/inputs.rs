//! Resolve build inputs without assembling or executing them.
mod record;
mod source_tests;

use crate::domain::{error::codes, root::RootLock, Arch, Error, Plan, RecipeBundle};
use crate::{environment::Environment, ApplicationConfig, BootstrapEnvironment, BuildRoot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Exact dependency inputs. Root directories and runner assets are execution state.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedInputs {
    pub root: Option<RootLock>,
    #[serde(default)]
    pub tests: Option<BTreeMap<String, RootLock>>,
    pub sysroot: Option<RootLock>,
    pub cross: Option<crate::domain::guest::CrossToolchain>,
}

impl PreparedInputs {
    pub fn for_source(
        bundle: &RecipeBundle,
        plan: &Plan,
        target: Arch,
        config: &ApplicationConfig,
    ) -> Result<Self, Error> {
        let mut inputs = match &config.root {
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
                    ..Self::default()
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
                    ..Self::default()
                }
            }
        };
        // Automatic cache receipts must not turn package resolution into a tree pin.
        // Explicit roots retain their caller-supplied filesystem verification.
        if matches!(config.root, BuildRoot::Automatic { .. }) {
            for root in inputs.root.iter_mut().chain(inputs.sysroot.iter_mut()) {
                root.root_digest = None;
                root.rpmdb_digest = None;
            }
        }
        if let Some(root) = &inputs.root {
            root.require_target(inputs.exec(target)?.as_str())?;
        }
        if let Some(root) = &inputs.sysroot {
            root.require_target(target.as_str())?;
        }
        Ok(inputs)
    }

    /// Every build caller selects its test scope before any environment executes.
    pub fn resolve(
        bundle: &RecipeBundle,
        plan: &Plan,
        target: Arch,
        config: &ApplicationConfig,
        selection: TestSelection,
    ) -> Result<Self, PreparationFailure> {
        let mut inputs =
            Self::for_source(bundle, plan, target, config).map_err(PreparationFailure::Root)?;
        inputs.complete(plan, selection, config)?;
        Ok(inputs)
    }

    pub fn complete(
        &mut self,
        plan: &Plan,
        selection: TestSelection,
        config: &ApplicationConfig,
    ) -> Result<(), PreparationFailure> {
        match selection {
            TestSelection::Disabled => Ok(()),
            TestSelection::Required => self
                .prepare_tests(plan, false, config)
                .map_err(PreparationFailure::Test),
            TestSelection::All => self
                .prepare_tests(plan, true, config)
                .map_err(PreparationFailure::Test),
        }
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
        Environment::from_inputs(self, target, config, true)
    }
    /// Replay uses the same materializer, with explicit runner-asset authority.
    pub fn replay(
        &self,
        target: Arch,
        config: &ApplicationConfig,
        allow_assets: bool,
    ) -> Result<Environment, Error> {
        Environment::from_inputs(self, target, config, allow_assets)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum TestSelection {
    Disabled,
    Required,
    All,
}
impl TestSelection {
    pub fn new(enabled: bool, optional: bool) -> Self {
        match (enabled, optional) {
            (false, _) => Self::Disabled,
            (true, false) => Self::Required,
            (true, true) => Self::All,
        }
    }
}

pub(crate) enum PreparationFailure {
    Root(Error),
    Test(Box<source_tests::TestInputFailure>),
}
impl PreparationFailure {
    pub fn report(self, report: &mut crate::RunReport, output: &std::path::Path) -> Error {
        match self {
            Self::Root(error) => error,
            Self::Test(failure) => failure.report(report, output),
        }
    }
}

/// Resolve and export portable inputs without source acquisition or execution.
pub(crate) fn execute(
    config: &crate::ApplicationConfig,
    input: &crate::ResolveInputsInput,
    output: &std::path::Path,
    report: &mut crate::RunReport,
) -> Result<(), rpmx_base::Error> {
    let target = crate::environment::target(&input.recipe, input.arch.as_deref(), config)?;
    let policy = crate::environment::policy(config)?;
    let (bundle, plan) =
        crate::environment::plan(&input.recipe, target, config, policy.plan_policy())?;
    let inputs = PreparedInputs::resolve(
        &bundle,
        &plan,
        target,
        config,
        TestSelection::new(true, input.include_optional_tests),
    )
    .map_err(|failure| failure.report(report, output))?;
    report
        .evidence
        .push(inputs.export(output, bundle.digest(), &plan.arch)?);
    Ok(())
}
