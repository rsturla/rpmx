//! Source-test selection and pinned build-root extensions belong to input preparation.
use super::*;
use crate::domain::root::RootSelection;
use rpmx_language::PlannedTest;
use std::collections::BTreeSet;

impl PreparedInputs {
    pub(super) fn prepare_tests(
        &mut self,
        plan: &Plan,
        include_optional: bool,
        config: &ApplicationConfig,
    ) -> Result<(), Box<TestInputFailure>> {
        let Some(baseline) = &self.root else {
            return Ok(());
        };
        let mut cases = BTreeMap::new();
        for test in plan
            .cells
            .iter()
            .flat_map(|cell| &cell.build_tests)
            .filter(|test| {
                test.skip.is_none() && crate::selection::includes(test, &[], include_optional)
            })
        {
            let result = (|| -> Result<Option<RootLock>, Error> {
                verify_target_requirements(test, self.sysroot.as_ref().unwrap_or(baseline))?;
                let extra = extra_packages(test, baseline);
                if extra.is_empty() {
                    return Ok(None);
                }
                if config.offline {
                    return Err(Error::new(codes::TEST_005,
                    "source-test dependencies are not cached in the prepared inputs; resolve them online first"));
                }
                let cache = config
                    .cache
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(crate::support::default_cache)?;
                let work = crate::support::WorkDir::new("resolve-source-tests")?;
                crate::domain::root::extend(baseline, &extra, work.path(), &cache).map(Some)
            })();
            let root = result.map_err(|error| {
                Box::new(TestInputFailure {
                    test: test.clone(),
                    error,
                    target: plan.arch.clone(),
                    exec: baseline.archs[0].clone(),
                })
            })?;
            if let Some(root) = root {
                cases.insert(
                    case_key(test).map_err(|error| {
                        Box::new(TestInputFailure {
                            test: test.clone(),
                            error,
                            target: plan.arch.clone(),
                            exec: baseline.archs[0].clone(),
                        })
                    })?,
                    root,
                );
            }
        }
        self.tests = Some(cases);
        Ok(())
    }

    pub fn test_root(
        &self,
        test: &PlannedTest,
        config: &ApplicationConfig,
    ) -> Result<Option<RootSelection>, Error> {
        let baseline = self.root.as_ref().ok_or_else(|| {
            Error::new(codes::TEST_005,
            "build tests require an exact pinned build closure; the host root cannot supply one")
        })?;
        verify_target_requirements(test, self.sysroot.as_ref().unwrap_or(baseline))?;
        if extra_packages(test, baseline).is_empty() {
            return Ok(None);
        }
        let lock = self.tests.as_ref().and_then(|cases| cases.get(&case_key(test).ok()?))
            .ok_or_else(|| Error::new(codes::TEST_005, format!("source-test environment for {} is not cached in the prepared inputs; prepare inputs with the same test selection", test.qualified_name())))?;
        verify_extension(baseline, lock)?;
        let roots = crate::autoroot::ensure_from_lock(
            lock,
            config.cache.as_deref(),
            config.offline,
            &lock.archs[0],
            false,
            None,
            false,
            false,
        )?;
        Ok(Some(RootSelection::verify_inputs(
            lock,
            &roots.root,
            &lock.archs[0],
        )?))
    }
}

fn case_key(test: &PlannedTest) -> Result<String, Error> {
    let bytes =
        serde_json::to_vec(test).map_err(|error| Error::new(codes::TEST_004, error.to_string()))?;
    Ok(crate::domain::hash::sha512_hex(&bytes))
}
fn extra_packages(test: &PlannedTest, baseline: &RootLock) -> Vec<String> {
    additions(test)
        .into_iter()
        .filter(|name| {
            name.split_whitespace().count() != 1
                || !baseline
                    .packages
                    .values()
                    .flatten()
                    .any(|package| package.name == *name)
        })
        .collect()
}
fn additions(test: &PlannedTest) -> BTreeSet<String> {
    test.packages
        .iter()
        .cloned()
        .chain(test.pipeline.iter().flat_map(|step| {
            if step.needs_packages.is_empty() && step.needs_sysroot.is_empty() {
                step.needs.clone()
            } else {
                step.needs_packages.clone()
            }
        }))
        .collect()
}
fn verify_target_requirements(test: &PlannedTest, target: &RootLock) -> Result<(), Error> {
    for required in test.pipeline.iter().flat_map(|step| &step.needs_sysroot) {
        if required.split_whitespace().count() != 1
            || !target
                .packages
                .values()
                .flatten()
                .any(|package| package.name == *required)
        {
            return Err(Error::new(codes::TEST_005, format!("target requirement '{required}' cannot be verified against the pinned sysroot; declare it in build.sysroot and resolve that closure before testing")));
        }
    }
    Ok(())
}

pub(super) fn verify_extension(baseline: &RootLock, extended: &RootLock) -> Result<(), Error> {
    let identity = |lock: &RootLock| {
        serde_json::to_value((
            &lock.archs,
            &lock.releasever,
            &lock.prefix,
            &lock.libdir,
            &lock.options,
            lock.builder.as_ref().map(|builder| &builder.installer),
        ))
    };
    if identity(extended).map_err(|error| Error::new(codes::TEST_004, error.to_string()))?
        != identity(baseline).map_err(|error| Error::new(codes::TEST_004, error.to_string()))?
    {
        return Err(Error::new(
            codes::TEST_005,
            "source-test environment changed pinned execution policy",
        ));
    }
    for (arch, packages) in &baseline.packages {
        for package in packages {
            if !extended.packages.get(arch).is_some_and(|resolved| {
                resolved.iter().any(|entry| {
                    entry.name == package.name
                        && entry.evr == package.evr
                        && entry.checksum == package.checksum
                })
            }) {
                return Err(Error::new(
                    codes::TEST_005,
                    "source-test environment changed the pinned build closure",
                ));
            }
        }
    }
    Ok(())
}

/// A resolution failure retains case identity without coupling resolution to execution.
pub(crate) struct TestInputFailure {
    test: PlannedTest,
    error: Error,
    target: String,
    exec: String,
}
impl TestInputFailure {
    pub fn report(
        mut self: Box<Self>,
        report: &mut crate::RunReport,
        output: &std::path::Path,
    ) -> Error {
        self.error.message = format!(
            "build test {} preparation: {}",
            self.test.qualified_name(),
            self.error.message
        );
        let evidence = rpmx_runner::test_runner::result_dir(
            &report.evidence_directory(output).join("build-tests"),
            &self.test,
        );
        let _ = std::fs::create_dir_all(&evidence);
        let _ = std::fs::write(
            evidence.join("preparation.log"),
            format!("{}\n", self.error),
        );
        report.environments.push(crate::EnvironmentRecord {
            id: crate::domain::hash::sha512_hex(self.test.qualified_name().as_bytes()),
            lock: evidence.join("root.lock.yaml"),
            assembly_identity: String::new(),
            root_digest: None,
            status: crate::RunOutcome::Failed,
            primary: Some(crate::Diagnostic::from(&self.error)),
        });
        report.build_tests.push(crate::CaseOutcome {
            target_arch: self.target,
            execution_arch: self.exec,
            name: self.test.name,
            cell_id: self.test.cell_id,
            output: self.test.output,
            status: crate::CaseStatus::Failed,
            reason: Some("environment preparation failed".into()),
            duration_ms: 0,
            evidence: evidence.clone(),
            primary: Some(crate::Diagnostic::from(&self.error)),
            secondary: Vec::new(),
        });
        report.evidence.push(evidence);
        self.error
    }
}
