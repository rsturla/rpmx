use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::plan::PlannedCell;
use rpmx_core::root::RootLock;
use rpmx_core::runner::RunnerKind;
use rpmx_core::sandbox::Sandbox;
use rpmx_core::test_root::{prepare as prepare_test_root, TestRootOptions};
use rpmx_core::test_runner::{run_planned_tests, TestSandboxConfig};
use rpmx_core::Error;

pub(super) struct TestRunOptions<'a> {
    pub(super) sandbox: &'a Sandbox<'a>,
    pub(super) cell: &'a PlannedCell,
    pub(super) written: &'a [PathBuf],
    pub(super) output: &'a Path,
    pub(super) work: &'a Path,
    pub(super) build: &'a Path,
    pub(super) stage: &'a Path,
    pub(super) active_lock: Option<&'a RootLock>,
    pub(super) builder_image: Option<&'a str>,
    pub(super) pinned_lock: Option<&'a Path>,
    pub(super) arch: &'a str,
    pub(super) runner: RunnerKind,
    pub(super) source_date_epoch: u32,
    pub(super) jobs: NonZeroU32,
    pub(super) timeout: NonZeroU64,
    pub(super) debuginfo: bool,
    pub(super) filter: &'a [String],
    pub(super) junit: Option<&'a Path>,
}

/// Run build- and installed-stage tests and persist the test-root lock.
pub(super) fn run(options: TestRunOptions<'_>) -> Result<Option<PathBuf>, Error> {
    let tests: Vec<_> = options
        .cell
        .tests
        .iter()
        .filter(|test| options.filter.is_empty() || options.filter.contains(&test.name))
        .cloned()
        .collect();
    if !options.filter.is_empty() && tests.is_empty() {
        return Err(Error::new(
            codes::TEST_004,
            format!("no test matches '{}'", options.filter.join(", ")),
        ));
    }
    let cache = crate::commands::fetch::default_cache()?;
    let prepared = if tests.iter().any(|test| test.stage == "installed") {
        let lock = options.active_lock.ok_or_else(|| {
            Error::new(
                codes::TEST_005,
                "installed-stage tests need an embedded or explicit root lock",
            )
        })?;
        Some(prepare_test_root(TestRootOptions {
            lock,
            pinned: options.pinned_lock,
            tools: &options.cell.test_tools,
            rpms: options.written,
            image: options.builder_image,
            cache: &cache,
            work: options.work,
            runner: options.runner,
        })?)
    } else {
        None
    };
    let test_root = prepared.as_ref().map(|root| root.path.as_path());
    let settings = TestSandboxConfig {
        workspace: options.build,
        stage: options.stage,
        host_arch: rpmx_core::arch::Arch::current()?,
        target_arch: rpmx_core::arch::Arch::parse(options.arch)?,
        runner: options.runner,
        source_date_epoch: options.source_date_epoch,
        jobs: options.jobs,
        timeout: options.timeout,
        debuginfo: options.debuginfo,
    };
    let mut outcomes = Vec::new();
    for test in &tests {
        let started = std::time::Instant::now();
        let result = run_planned_tests(
            options.sandbox,
            std::slice::from_ref(test),
            test_root,
            settings,
        );
        let error = result.as_ref().err().map(|error| error.message.clone());
        eprintln!(
            "rpmx: test {} ({}): {} ({:.1}s)",
            test.name,
            test.stage,
            if error.is_some() { "FAILED" } else { "ok" },
            started.elapsed().as_secs_f64()
        );
        outcomes.push((
            test.name.clone(),
            test.stage.clone(),
            error,
            started.elapsed(),
        ));
    }
    let failed: Vec<_> = outcomes
        .iter()
        .filter(|(_, _, error, _)| error.is_some())
        .collect();
    if let Some(path) = options.junit {
        write_junit(path, &outcomes).map_err(|error| {
            Error::new(
                codes::TEST_003,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }
    let evidence = if let Some(prepared) = prepared {
        let _ = rpmx_core::root::remove_tree(&prepared.path);
        let path = options.output.join(format!(
            "{}-{}-{}.test-root.lock.json",
            options.cell.package, options.cell.version, options.cell.release
        ));
        let text = serde_yaml_ng::to_string(&prepared.lock)
            .map_err(|error| Error::new(codes::TEST_003, error.to_string()))?;
        std::fs::write(&path, text).map_err(|error| {
            Error::new(
                codes::TEST_003,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
        Some(path)
    } else {
        None
    };
    if !failed.is_empty() {
        return Err(
            Error::new(codes::TEST_003, format!("{} test(s) failed", failed.len())).reported(),
        );
    }
    let _ = evidence;
    Ok(evidence)
}

fn write_junit(
    path: &Path,
    outcomes: &[(String, String, Option<String>, std::time::Duration)],
) -> std::io::Result<()> {
    let failures = outcomes
        .iter()
        .filter(|(_, _, error, _)| error.is_some())
        .count();
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"rpmx\" tests=\"{}\" failures=\"{}\">\n",
        outcomes.len(),
        failures
    );
    for (name, stage, error, duration) in outcomes {
        xml.push_str(&format!(
            "  <testcase name=\"{}\" classname=\"{}\" time=\"{:.3}\">",
            escape(name),
            escape(stage),
            duration.as_secs_f64()
        ));
        if let Some(error) = error {
            xml.push_str(&format!(
                "<failure message=\"{}\">{}</failure>",
                escape(error),
                escape(error)
            ));
        }
        xml.push_str("</testcase>\n");
    }
    xml.push_str("</testsuite>\n");
    std::fs::write(path, xml)
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::write_junit;
    use std::time::Duration;

    #[test]
    fn junit_escapes_messages_and_counts_failures() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("report.xml");
        write_junit(
            &path,
            &[
                (
                    "ok <test>".to_string(),
                    "build".to_string(),
                    None,
                    Duration::from_millis(10),
                ),
                (
                    "bad".to_string(),
                    "installed".to_string(),
                    Some("a & b".to_string()),
                    Duration::from_millis(20),
                ),
            ],
        )
        .expect("junit");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("tests=\"2\""), "{text}");
        assert!(text.contains("failures=\"1\""), "{text}");
        assert!(text.contains("ok &lt;test&gt;"), "{text}");
        assert!(text.contains("a &amp; b"), "{text}");
    }
}
