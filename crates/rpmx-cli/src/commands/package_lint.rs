//! Checks that need the assembled package, not just the recipe.

use rpmx_core::error::codes;
use rpmx_core::package_spec::BinaryPackageSpec;
use rpmx_core::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub severity: Severity,
    pub message: String,
}

/// Inspect a built package's files.
pub fn check(packages: &[BinaryPackageSpec]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for package in packages {
        if package.files.is_empty() {
            continue;
        }
        let has_license = package.files.iter().any(|file| file.license);
        for file in &package.files {
            findings.extend(check_file(
                &file.path,
                file.mode,
                file.license,
                file.target.is_some(),
            ));
        }
        if !has_license {
            findings.push(Finding {
                path: package.name.clone(),
                severity: Severity::Warning,
                message: format!(
                    "package '{}' ships no file marked as a license; add files.license",
                    package.name
                ),
            });
        }
    }
    findings
}

/// The per-path rules, exposed for tests.
pub fn check_file(path: &str, mode: u32, license: bool, symlink: bool) -> Vec<Finding> {
    let mut findings = Vec::new();
    if !symlink && mode & 0o002 != 0 {
        findings.push(Finding {
            path: path.to_string(),
            severity: Severity::Error,
            message: format!("{path} is world-writable (mode {mode:04o})"),
        });
    }
    if !symlink && mode & 0o6000 != 0 {
        findings.push(Finding {
            path: path.to_string(),
            severity: Severity::Error,
            message: format!("{path} is setuid/setgid (mode {mode:04o})"),
        });
    }
    if path.starts_with("/tmp/") || path.starts_with("/var/empty/") {
        findings.push(Finding {
            path: path.to_string(),
            severity: Severity::Error,
            message: format!("{path} must not be shipped in a temporary directory"),
        });
    }
    if path.starts_with("/usr/local/") || path.starts_with("/opt/") {
        findings.push(Finding {
            path: path.to_string(),
            severity: Severity::Warning,
            message: format!("{path} is outside the distribution prefix (/usr)"),
        });
    }
    if path.ends_with(".la") {
        findings.push(Finding {
            path: path.to_string(),
            severity: Severity::Warning,
            message: format!("{path} is a libtool archive"),
        });
    }
    let _ = license;
    findings
}

/// Print findings and fail on errors.
pub fn enforce(findings: &[Finding]) -> Result<(), Error> {
    for finding in findings {
        match finding.severity {
            Severity::Error => {
                rpmx_core::output::error(format!("lint: {} [RPMX-POLICY-001]", finding.message))
            }
            Severity::Warning => {
                rpmx_core::output::warning("lint", format!("{} [RPMX-POLICY-001]", finding.message))
            }
        }
    }
    let errors = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .count();
    if errors > 0 {
        return Err(Error::new(
            codes::POLICY_001,
            format!("{errors} package policy violation(s)"),
        )
        .reported());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_file, Severity};

    #[test]
    fn world_writable_and_setuid_files_fail() {
        assert!(check_file("/usr/bin/x", 0o777, false, false)
            .iter()
            .any(|finding| finding.severity == Severity::Error));
        assert!(check_file("/usr/bin/x", 0o4755, false, false)
            .iter()
            .any(|finding| finding.severity == Severity::Error));
    }

    #[test]
    fn temporary_and_non_usr_prefixes_are_reported() {
        assert!(check_file("/tmp/x", 0o644, false, false)
            .iter()
            .any(|finding| finding.severity == Severity::Error));
        assert!(check_file("/opt/x", 0o644, false, false)
            .iter()
            .any(|finding| finding.severity == Severity::Warning));
    }

    #[test]
    fn ordinary_files_pass() {
        assert!(check_file("/usr/bin/x", 0o755, false, false).is_empty());
    }
}
