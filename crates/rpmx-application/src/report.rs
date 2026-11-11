use rpmx_base::{error::codes, Error};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunOutcome {
    Running,
    Passed,
    Failed,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
}
impl From<&Error> for Diagnostic {
    fn from(error: &Error) -> Self {
        Self {
            code: error.code.to_string(),
            message: error.message.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtifactSet {
    pub rpms: Vec<PathBuf>,
    pub srpms: Vec<PathBuf>,
    pub sources: Vec<PathBuf>,
    pub repository: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcceptanceState {
    NotRequested,
    Running,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaseStatus {
    Passed,
    Failed,
    Skipped,
    NotRun,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseOutcome {
    pub name: String,
    pub cell_id: String,
    pub output: Option<String>,
    pub target_arch: String,
    pub execution_arch: String,
    pub status: CaseStatus,
    pub reason: Option<String>,
    pub duration_ms: u64,
    pub evidence: PathBuf,
    pub primary: Option<Diagnostic>,
    pub secondary: Vec<Diagnostic>,
}

impl CaseOutcome {
    pub(crate) fn from_execution(
        test: &rpmx_language::PlannedTest,
        result: rpmx_runner::test_runner::TestExecutionReport,
    ) -> Self {
        use rpmx_runner::test_runner::TestStatus;
        Self {
            name: test.name.clone(),
            cell_id: test.cell_id.clone(),
            output: test.output.clone(),
            target_arch: result.target_arch.to_string(),
            execution_arch: result.execution_arch.to_string(),
            status: match result.status {
                TestStatus::Passed => CaseStatus::Passed,
                TestStatus::Failed => CaseStatus::Failed,
                TestStatus::Skipped => CaseStatus::Skipped,
            },
            reason: result.reason,
            duration_ms: result.duration.as_millis().min(u64::MAX as u128) as u64,
            evidence: result.evidence,
            primary: result.primary.as_ref().map(Diagnostic::from),
            secondary: result.secondary.iter().map(Diagnostic::from).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputArtifact {
    pub name: String,
    pub evr: String,
    pub arch: String,
    pub checksum: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentRecord {
    pub id: String,
    pub lock: PathBuf,
    pub assembly_identity: String,
    pub root_digest: Option<String>,
    pub status: RunOutcome,
    pub primary: Option<Diagnostic>,
}

/// One invocation owns its evidence before it prepares resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub document: String,
    pub version: u32,
    pub id: String,
    pub operation: String,
    pub outcome: RunOutcome,
    pub artifacts: ArtifactSet,
    pub evidence: Vec<PathBuf>,
    pub build_tests_requested: bool,
    pub build_tests: Vec<CaseOutcome>,
    pub acceptance: AcceptanceState,
    pub tests: Vec<CaseOutcome>,
    pub inputs: Vec<InputArtifact>,
    pub environments: Vec<EnvironmentRecord>,
    pub primary: Option<Diagnostic>,
    pub secondary: Vec<Diagnostic>,
}

impl RunReport {
    pub(crate) fn new(operation: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            document: "rpmx-run-report".into(),
            version: 1,
            id: format!(
                "{time:x}-{:x}-{:x}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            operation: operation.into(),
            outcome: RunOutcome::Running,
            artifacts: ArtifactSet::default(),
            evidence: Vec::new(),
            build_tests_requested: false,
            build_tests: Vec::new(),
            acceptance: AcceptanceState::NotRequested,
            tests: Vec::new(),
            inputs: Vec::new(),
            environments: Vec::new(),
            primary: None,
            secondary: Vec::new(),
        }
    }
    pub(crate) fn evidence_directory(&self, output: &Path) -> PathBuf {
        output.join("evidence").join(&self.id)
    }
    pub fn fail(&mut self, error: &Error) {
        self.outcome = RunOutcome::Failed;
        let diagnostic = Diagnostic::from(error);
        if self.primary.is_none() {
            self.primary = Some(diagnostic);
        } else {
            self.secondary.push(diagnostic);
        }
    }
    pub(crate) fn complete(&mut self) {
        self.outcome = if self.primary.is_some() {
            RunOutcome::Failed
        } else if self
            .build_tests
            .iter()
            .chain(&self.tests)
            .any(|case| matches!(case.status, CaseStatus::NotRun | CaseStatus::Skipped))
        {
            RunOutcome::Incomplete
        } else {
            RunOutcome::Passed
        };
    }
    pub fn success(&self) -> bool {
        self.outcome == RunOutcome::Passed
    }
    pub fn write(&self, output: &Path) -> Result<PathBuf, Error> {
        use std::io::Write;
        let path = output.join("run-report.json");
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(output)?;
            let mut file = tempfile::NamedTempFile::new_in(output)?;
            serde_json::to_writer_pretty(&mut file, self)?;
            file.write_all(b"\n")?;
            file.persist(&path).map_err(|error| error.error)?;
            Ok(())
        };
        write().map_err(|error| {
            Error::new(
                codes::BUILD_006,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
        self.write_junit(&output.join("run-report.xml"))?;
        Ok(path)
    }
    /// Replace this run's report; never append cases from an earlier invocation.
    pub fn write_junit(&self, path: &Path) -> Result<(), Error> {
        use std::io::Write;
        let mut cases = String::new();
        let mut count = 0;
        let mut failed = 0;
        let mut errors = 0;
        let mut skipped = 0;
        for (phase, results) in [("build", &self.build_tests), ("package", &self.tests)] {
            for case in results {
                count += 1;
                cases.push_str(&format!(
                    "<testcase name=\"{}\" classname=\"{}.{}.{}.{}\" time=\"{:.3}\">",
                    xml(&case
                        .output
                        .as_ref()
                        .map(|output| format!("{output}/{}", case.name))
                        .unwrap_or_else(|| case.name.clone())),
                    phase,
                    xml(&case.target_arch),
                    xml(&case.execution_arch),
                    xml(&case.cell_id),
                    case.duration_ms as f64 / 1000.0
                ));
                match case.status {
                    CaseStatus::Failed => {
                        failed += 1;
                        let message = case
                            .primary
                            .as_ref()
                            .map(|diagnostic| diagnostic.message.as_str())
                            .unwrap_or("test failed");
                        cases.push_str(&format!(
                            "<failure message=\"{}\">{}</failure>",
                            xml(message),
                            xml(message)
                        ));
                    }
                    CaseStatus::Skipped | CaseStatus::NotRun => {
                        skipped += 1;
                        cases.push_str(&format!(
                            "<skipped message=\"{}\"/>",
                            xml(case.reason.as_deref().unwrap_or("test was not run"))
                        ));
                    }
                    CaseStatus::Passed => {}
                }
                let mut detail = format!("Evidence: {}", case.evidence.display());
                for diagnostic in &case.secondary {
                    detail.push_str(&format!("\n{}: {}", diagnostic.code, diagnostic.message));
                }
                cases.push_str(&format!(
                    "<system-out>{}</system-out></testcase>\n",
                    xml(&detail)
                ));
            }
        }
        // Acquisition, planning and environment errors still produce a JUnit case.
        if let Some(diagnostic) = &self.primary {
            if failed == 0 {
                count += 1;
                errors += 1;
                cases.push_str(&format!("<testcase name=\"preparation\" classname=\"{}\"><error message=\"{}\">{}</error></testcase>\n", xml(&self.operation), xml(&diagnostic.message), xml(&diagnostic.message)));
            }
        }
        let document = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"{}\" tests=\"{count}\" failures=\"{failed}\" errors=\"{errors}\" skipped=\"{skipped}\">\n{cases}</testsuite>\n", xml(&self.operation));
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(parent)?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            temporary.write_all(document.as_bytes())?;
            temporary.persist(path).map_err(|error| error.error)?;
            Ok(())
        };
        write().map_err(|error| {
            Error::new(
                codes::TEST_004,
                format!("cannot write JUnit report {}: {error}", path.display()),
            )
        })
    }
    pub fn require_success(&self) -> Result<(), Error> {
        if self.success() {
            return Ok(());
        }
        let message = self.primary.as_ref().map_or_else(
            || "run did not complete".into(),
            |error| format!("{}: {}", error.code, error.message),
        );
        Err(Error::new(codes::BUILD_006, message))
    }
}

fn xml(value: &str) -> String {
    value
        .chars()
        .filter(|ch| matches!(ch, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
