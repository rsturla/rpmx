use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

use crate::sandbox::Sandbox;
use rpmx_base::error::{codes, Error, Result};
use rpmx_language::plan::PlannedStep;

/// The package name part of a dependency string such as `gcc >= 12`.
pub fn dependency_name(value: &str) -> &str {
    value.split_whitespace().next().unwrap_or(value)
}

/// Fail before the first step if a declared build tool is neither a package in
/// the locked root nor an executable inside it. Version constraints are parsed
/// off the name and are not checked yet.
pub fn check_build_tools(
    sandbox: &Sandbox<'_>,
    workspace: &Path,
    packages: &[String],
    sysroot: &[String],
    locked_packages: &BTreeSet<String>,
) -> Result<()> {
    let mut missing_packages = BTreeSet::new();
    for dependency in sysroot {
        let name = dependency_name(dependency);
        if !name.is_empty() && !locked_packages.contains(name) {
            missing_packages.insert(name.to_string());
        }
    }
    if !missing_packages.is_empty() && !locked_packages.is_empty() {
        return Err(Error::new(
            codes::ROOT_018,
            format!(
                "sysroot packages missing from the selected root: {}; \
                 add them to the profile's packages.install",
                missing_packages.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }

    let mut candidates = BTreeSet::new();
    for tool in packages {
        let name = dependency_name(tool);
        if name.is_empty() || locked_packages.contains(name) {
            continue;
        }
        candidates.insert(name.to_string());
    }
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
    args.extend(candidates.iter().cloned());
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
                "build tools missing from the selected root: {}; add them to the profile's packages.install",
                missing.join(", ")
            ),
        ))
    }
}
