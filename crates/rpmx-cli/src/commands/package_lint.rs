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
    for (index, package) in packages.iter().enumerate() {
        if index == 0 {
            check_main_package(package, &mut findings);
        }
        for require in &package.dependencies.requires {
            if require.starts_with("bundled(") {
                findings.push(Finding {
                    path: package.name.clone(),
                    severity: Severity::Error,
                    message: format!(
                        "package '{}' requires '{require}'; bundled() is a provide, not a dependency",
                        package.name
                    ),
                });
            }
        }
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

type FileCategory = (&'static str, &'static str, &'static str, fn(&str) -> bool);

fn check_main_package(package: &BinaryPackageSpec, findings: &mut Vec<Finding>) {
    let categories: &[FileCategory] = &[
        (
            "development header",
            "development headers",
            "use files.include: [\"@headers\"] or [\"@dev\"]",
            |path| path.starts_with("/usr/include/"),
        ),
        (
            "static library",
            "static libraries",
            "use files.include: [\"@static\"]",
            |path| is_libdir_file(path) && path.ends_with(".a"),
        ),
        (
            "pkg-config or CMake file",
            "pkg-config or CMake files",
            "use files.include: [\"@pkgconfig\"] or [\"@dev\"]",
            |path| {
                (is_libdir_path(path) && (path.contains("/pkgconfig/") || path.contains("/cmake/")))
                    || path.starts_with("/usr/share/pkgconfig/")
                    || path.starts_with("/usr/share/cmake/")
            },
        ),
        (
            "documentation file",
            "documentation files",
            "use files.include: [\"@doc\", \"@info\", \"@man\"]",
            |path| {
                path.starts_with("/usr/share/man/")
                    || path.starts_with("/usr/share/info/")
                    || path.starts_with("/usr/share/doc/")
                    || path.starts_with("/usr/share/gtk-doc/")
                    || path.starts_with("/usr/share/help/")
            },
        ),
        (
            "locale source file",
            "locale source files",
            "use files.include: [\"@locales\"]",
            |path| path.starts_with("/usr/share/i18n/"),
        ),
    ];

    for (singular, plural, advice, matches) in categories {
        let matching = package.files.iter().filter(|file| matches(&file.path));
        let count = matching.clone().count();
        if count == 0 {
            continue;
        }
        let noun = if count == 1 { singular } else { plural };
        let examples = matching
            .take(3)
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        findings.push(Finding {
            path: package.name.clone(),
            severity: Severity::Warning,
            message: format!(
                "main package '{}' ships {count} {noun} (for example {examples}); {advice}",
                package.name
            ),
        });
    }
}

fn is_libdir_path(path: &str) -> bool {
    path.starts_with("/usr/lib/") || path.starts_with("/usr/lib64/")
}

fn is_libdir_file(path: &str) -> bool {
    is_libdir_path(path)
        && path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("lib"))
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
    use super::{check, check_file, check_main_package, Severity};
    use rpmx_core::package_spec::{BinaryPackageFile, BinaryPackageSpec};

    #[test]
    fn a_bundled_provide_must_not_be_required() {
        let package = BinaryPackageSpec {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            release: "1".to_string(),
            epoch: None,
            arch: "x86_64".to_string(),
            summary: "demo".to_string(),
            license: "MIT".to_string(),
            description: "demo".to_string(),
            url: None,
            source_rpm: "demo-1.0-1.src.rpm".to_string(),
            source_date_epoch: 1,
            files: Vec::new(),
            directories: Vec::new(),
            dependencies: rpmx_core::model::DependencySet {
                requires: vec!["bundled(zlib)".to_string()],
                ..Default::default()
            },
            scriptlets: Default::default(),
        };
        assert!(check(&[package])
            .iter()
            .any(|finding| finding.severity == Severity::Error));
    }

    #[test]
    fn the_main_package_reports_development_files_with_a_preset_hint() {
        let package = BinaryPackageSpec {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            release: "1".to_string(),
            epoch: None,
            arch: "x86_64".to_string(),
            summary: "demo".to_string(),
            license: "MIT".to_string(),
            description: "demo".to_string(),
            url: None,
            source_rpm: "demo-1.0-1.src.rpm".to_string(),
            source_date_epoch: 1,
            files: vec![BinaryPackageFile {
                config: false,
                noreplace: false,
                doc: false,
                license: false,
                path: "/usr/include/demo.h".to_string(),
                data: Vec::new(),
                mode: 0o644,
                target: None,
                user: None,
                group: None,
                caps: None,
            }],
            directories: Vec::new(),
            dependencies: rpmx_core::model::DependencySet::default(),
            scriptlets: Default::default(),
        };
        let mut findings = Vec::new();
        check_main_package(&package, &mut findings);
        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Warning && finding.message.contains("@headers")
        }));
    }

    #[test]
    fn the_main_package_reports_static_archives_with_the_static_preset() {
        let package = BinaryPackageSpec {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            release: "1".to_string(),
            epoch: None,
            arch: "x86_64".to_string(),
            summary: "demo".to_string(),
            license: "MIT".to_string(),
            description: "demo".to_string(),
            url: None,
            source_rpm: "demo-1.0-1.src.rpm".to_string(),
            source_date_epoch: 1,
            files: vec![BinaryPackageFile {
                config: false,
                noreplace: false,
                doc: false,
                license: false,
                path: "/usr/lib64/libdemo.a".to_string(),
                data: Vec::new(),
                mode: 0o644,
                target: None,
                user: None,
                group: None,
                caps: None,
            }],
            directories: Vec::new(),
            dependencies: rpmx_core::model::DependencySet::default(),
            scriptlets: Default::default(),
        };
        let mut findings = Vec::new();
        check_main_package(&package, &mut findings);
        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Warning
                && finding.message.contains("@static")
                && !finding.message.contains("@dev")
        }));
    }

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
