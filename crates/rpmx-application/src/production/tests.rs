//! Build tests use the standalone acceptance executor. Preparing the exact
//! build-root extension is the only production-specific execution policy.
use crate::domain::{
    error::codes,
    root::{RootLock, RootSelection},
    sandbox::SandboxConfig,
    Error, RecipeBundle,
};
use crate::{ApplicationConfig, CaseOutcome, CaseStatus, EnvironmentRecord, RunOutcome, RunReport};
use rpmx_language::PlannedTest;
use rpmx_runner::test_runner::{execute_test, TestEnvironment, WorkspaceSource};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(super) struct PreparedTests {
    cases: Vec<PreparedCase>,
    first: Option<Error>,
    target: rpmx_base::Arch,
    exec: rpmx_base::Arch,
}
struct PreparedCase {
    test: PlannedTest,
    root: Option<RootSelection>,
    failed: bool,
}

pub(super) fn prepare(
    config: &ApplicationConfig,
    cell: &crate::domain::plan::PlannedCell,
    baseline: Option<&RootLock>,
    target: Option<&RootLock>,
    template: SandboxConfig<'_>,
    output: &Path,
    report: &mut RunReport,
) -> PreparedTests {
    let evidence = report.evidence_directory(output).join("build-tests");
    let mut prepared = PreparedTests {
        cases: Vec::new(),
        first: None,
        target: template.target,
        exec: template.exec,
    };
    for test in &cell.build_tests {
        if test.skip.is_some() {
            if prepared.first.is_none() {
                prepared.first = Some(Error::new(codes::TEST_005, format!("build test {} is skipped; requested build tests must all pass before publishing", test.qualified_name())));
            }
            prepared.cases.push(PreparedCase {
                test: test.clone(),
                root: None,
                failed: false,
            });
            continue;
        }
        let started = std::time::Instant::now();
        let case_evidence = rpmx_runner::test_runner::result_dir(&evidence, test);
        let id = crate::domain::hash::sha512_hex(
            format!(
                "{}:{}:{}:{}",
                template.target,
                template.exec,
                cell.id,
                test.qualified_name()
            )
            .as_bytes(),
        )
        .trim_start_matches("sha512:")
        .to_string();
        let lock_path = evidence
            .join("environments")
            .join(format!("{id}.lock.yaml"));
        let position = report.environments.len();
        report.environments.push(EnvironmentRecord {
            id,
            lock: lock_path.clone(),
            assembly_identity: String::new(),
            root_digest: None,
            status: RunOutcome::Running,
            primary: None,
        });
        let result = (|| -> Result<Option<RootSelection>, Error> {
            std::fs::create_dir_all(&case_evidence).map_err(file_error)?;
            let baseline = baseline.ok_or_else(|| Error::new(codes::TEST_005, "build tests require an exact pinned build closure; the host root cannot supply one"))?;
            verify_target_requirements(test, target.unwrap_or(baseline))?;
            let mut additions = additions(test);
            // A bare package already present needs no new solve. Constraints
            // and virtual capabilities still go through the real solver.
            additions.retain(|name| {
                name.split_whitespace().count() != 1
                    || !baseline
                        .packages
                        .values()
                        .flatten()
                        .any(|package| package.name == *name)
            });
            let root = if additions.is_empty() {
                None
            } else {
                Some(extension(
                    config,
                    baseline,
                    &additions.into_iter().collect::<Vec<_>>(),
                )?)
            };
            let lock = root.as_ref().map(|root| root.lock()).unwrap_or(baseline);
            write_public_lock(&lock_path, lock)?;
            report.environments[position].assembly_identity = lock.assembly_identity();
            report.environments[position].root_digest = lock.root_digest.clone();
            report.evidence.push(lock_path.clone());
            Ok(root)
        })();
        match result {
            Ok(root) => {
                report.environments[position].status = RunOutcome::Passed;
                prepared.cases.push(PreparedCase {
                    test: test.clone(),
                    root,
                    failed: false,
                });
            }
            Err(mut error) => {
                error.message = format!(
                    "build test {} preparation: {}",
                    test.qualified_name(),
                    error.message
                );
                report.environments[position].status = RunOutcome::Failed;
                report.environments[position].primary = Some(crate::Diagnostic::from(&error));
                report.build_tests.push(CaseOutcome {
                    target_arch: template.target.to_string(),
                    execution_arch: template.exec.to_string(),
                    name: test.name.clone(),
                    cell_id: test.cell_id.clone(),
                    output: test.output.clone(),
                    status: CaseStatus::Failed,
                    reason: Some("environment preparation failed".into()),
                    duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    evidence: case_evidence.clone(),
                    primary: Some(crate::Diagnostic::from(&error)),
                    secondary: Vec::new(),
                });
                let _ = std::fs::write(case_evidence.join("preparation.log"), format!("{error}\n"));
                report.evidence.push(case_evidence);
                if prepared.first.is_none() {
                    prepared.first = Some(error);
                }
                prepared.cases.push(PreparedCase {
                    test: test.clone(),
                    root: None,
                    failed: true,
                });
            }
        }
    }
    prepared
}

impl PreparedTests {
    /// All independent environments are attempted, but a known setup failure
    /// prevents a long build whose complete test gate cannot succeed.
    pub fn require_ready(&self, report: &mut RunReport) -> Result<(), Error> {
        if let Some(error) = &self.first {
            for case in self.cases.iter().filter(|case| !case.failed) {
                report.build_tests.push(CaseOutcome {
                    target_arch: self.target.to_string(),
                    execution_arch: self.exec.to_string(),
                    name: case.test.name.clone(),
                    cell_id: case.test.cell_id.clone(),
                    output: case.test.output.clone(),
                    status: if case.test.skip.is_some() {
                        CaseStatus::Skipped
                    } else {
                        CaseStatus::NotRun
                    },
                    reason: Some(case.test.skip.clone().unwrap_or_else(|| {
                        "build did not start because another test environment failed".into()
                    })),
                    duration_ms: 0,
                    evidence: PathBuf::new(),
                    primary: None,
                    secondary: Vec::new(),
                });
            }
            return Err(error.clone());
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute(
    prepared: PreparedTests,
    bundle: &RecipeBundle,
    template: SandboxConfig<'_>,
    payload: &Path,
    candidates: &[PathBuf],
    output: &Path,
    report: &mut RunReport,
) -> Result<(), Error> {
    let evidence = report.evidence_directory(output).join("build-tests");
    let mut first = None;
    for case in prepared.cases {
        let mut sandbox = template;
        if let Some(root) = &case.root {
            sandbox.root = Some(root.path());
        }
        let execution = execute_test(
            &case.test,
            TestEnvironment {
                sandbox,
                workspace: WorkspaceSource::Copy(template.workspace),
                fixture_root: bundle.root.parent().expect("extracted recipe directory"),
                payload_root: payload,
                rpms: candidates,
            },
            &evidence,
        );
        if let Some(error) = &execution.primary {
            if first.is_none() {
                first = Some(error.clone());
            }
        }
        if execution.status != rpmx_runner::test_runner::TestStatus::Passed && first.is_none() {
            first = Some(Error::new(
                codes::TEST_005,
                format!(
                    "build test {} did not pass; candidate RPMs will not be published",
                    case.test.qualified_name()
                ),
            ));
        }
        report.evidence.push(execution.evidence.clone());
        report
            .build_tests
            .push(CaseOutcome::from_execution(&case.test, execution));
    }
    match first {
        Some(error) => Err(error),
        None => Ok(()),
    }
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
fn extension(
    config: &ApplicationConfig,
    baseline: &RootLock,
    additions: &[String],
) -> Result<RootSelection, Error> {
    let cache = config
        .cache
        .clone()
        .map(Ok)
        .unwrap_or_else(crate::support::default_cache)?;
    let material = serde_json::to_vec(&(
        1,
        baseline.assembly_identity(),
        &baseline.root_digest,
        additions,
    ))
    .map_err(|error| Error::new(codes::TEST_004, error.to_string()))?;
    let key = crate::domain::hash::sha512_hex(&material);
    let base = cache
        .parent()
        .unwrap_or(&cache)
        .join("build-test-environments");
    std::fs::create_dir_all(&base).map_err(file_error)?;
    let directory = base.join(key.trim_start_matches("sha512:"));
    let load = || -> Result<RootSelection, Error> {
        let root = RootSelection::load(
            &directory.join("root.lock.yaml"),
            &directory.join("root"),
            &baseline.archs[0],
        )?;
        let identity = |lock: &RootLock| {
            serde_json::to_value((
                &lock.archs,
                &lock.releasever,
                &lock.prefix,
                &lock.libdir,
                &lock.options,
                &lock.builder,
            ))
        };
        if identity(root.lock()).map_err(|error| Error::new(codes::TEST_004, error.to_string()))?
            != identity(baseline).map_err(|error| Error::new(codes::TEST_004, error.to_string()))?
        {
            return Err(Error::new(
                codes::TEST_005,
                "cached build-test environment changed pinned execution policy",
            ));
        }
        for (arch, packages) in &baseline.packages {
            for package in packages {
                if !root.lock().packages.get(arch).is_some_and(|resolved| {
                    resolved.iter().any(|entry| {
                        entry.name == package.name
                            && entry.evr == package.evr
                            && entry.checksum == package.checksum
                    })
                }) {
                    return Err(Error::new(
                        codes::TEST_005,
                        "cached build-test environment changed the pinned build closure",
                    ));
                }
            }
        }
        Ok(root)
    };
    if directory.exists() {
        return load();
    }
    if config.offline {
        return Err(Error::new(codes::TEST_005, format!("build-test environment {} is not cached; prepare it online before an offline rebuild", directory.display())));
    }
    let _publishing =
        crate::domain::cache_lock::CacheLock::acquire(&base, key.trim_start_matches("sha512:"))?;
    if directory.exists() {
        return load();
    }
    let temporary = tempfile::tempdir_in(&base).map_err(file_error)?;
    let work = temporary.path().join("prepare");
    std::fs::create_dir_all(&work).map_err(file_error)?;
    let lock = crate::domain::root::extend(baseline, additions, &work, &cache)?;
    let selected = RootSelection::assemble(
        lock,
        crate::domain::root::AssembleOptions {
            cache: &cache,
            work: &work,
            root: &temporary.path().join("root"),
        },
    )?;
    write_public_lock(&temporary.path().join("root.lock.yaml"), selected.lock())?;
    drop(selected);
    // Resolver scratch files may contain repository credentials. Only the
    // verified root and redacted binding enter the reusable cache.
    crate::domain::root::remove_tree(&work)?;
    let staged = temporary.keep();
    std::fs::rename(&staged, &directory).map_err(file_error)?;
    load()
}
fn write_public_lock(path: &Path, lock: &RootLock) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(file_error)?;
    }
    crate::domain::root::write_lock(path, &lock.redacted())
}
fn file_error(error: std::io::Error) -> Error {
    Error::new(codes::TEST_004, format!("build-test environment: {error}"))
}
