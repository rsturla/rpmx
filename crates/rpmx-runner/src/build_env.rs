use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

use crate::sandbox::Sandbox;
use rpmx_base::error::{codes, Error, Result};
use rpmx_language::plan::PlannedStep;

/// The package name part of a dependency string such as `gcc >= 12`.
fn dependency_name(value: &str) -> &str {
    value.split_whitespace().next().unwrap_or(value)
}

/// Probe executable names only for explicit unsafe-host execution.
/// RPM-managed roots validate capabilities from their pinned package inputs.
pub fn check_host_tools(
    sandbox: &Sandbox<'_>,
    workspace: &Path,
    packages: &[String],
) -> Result<()> {
    let candidates = packages
        .iter()
        .map(|tool| dependency_name(tool))
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>();
    if candidates.is_empty() {
        return Ok(());
    }

    let marker = workspace.join(".rpmx-missing-tools");
    let _ = std::fs::remove_file(&marker);
    let mut args = vec![
        "-c".to_string(),
        format!(
            "for tool do command -v \"$tool\" >/dev/null 2>&1 || printf '%s\n' \"$tool\"; done > {}/.rpmx-missing-tools",
            rpmx_base::guest::BUILD
        ),
        "sh".to_string(),
    ];
    args.extend(candidates.into_iter().map(str::to_string));
    let step = PlannedStep {
        label: "tool-probe".to_string(),
        uses: "tool-probe".to_string(),
        command: Some("sh".to_string()),
        args,
        cwd: None,
        timeout_seconds: None,
        env: BTreeMap::new(),
        needs: Vec::new(),
        needs_packages: Vec::new(),
        needs_sysroot: Vec::new(),
        tool_digest: None,
        step_digest: None,
        builtin: false,
        inputs: BTreeMap::new(),
        input: None,
        input_file: None,
        files: None,
        artifacts: Vec::new(),
    };
    sandbox.run(&step).map_err(|error| {
        Error::new(
            codes::ROOT_018,
            format!("cannot probe the build root for tools: {error}"),
        )
    })?;

    let report = std::fs::read_to_string(&marker).unwrap_or_default();
    let _ = std::fs::remove_file(&marker);
    let missing = report.lines().map(str::to_string).collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Error::new(
            codes::ROOT_018,
            format!(
                "build tools missing from the unsafe host: {}",
                missing.join(", ")
            ),
        ))
    }
}
