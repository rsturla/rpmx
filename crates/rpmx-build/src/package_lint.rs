//! Checks that need the assembled package, not just the recipe.

use std::collections::{BTreeMap, BTreeSet};

use crate::package_spec::BinaryPackageSpec;
use rpmx_base::error::codes;
use rpmx_base::Error;

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
    let packages_by_name: BTreeMap<&str, &BinaryPackageSpec> = packages
        .iter()
        .map(|package| (package.name.as_str(), package))
        .collect();
    let mut findings = Vec::new();
    for (index, package) in packages.iter().enumerate() {
        if index == 0 {
            check_main_package(package, &mut findings);
        }
        check_output_name(package, &mut findings);
        check_output_files(package, &mut findings);
        check_output_dependencies(package, &packages_by_name, &mut findings);
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

fn check_output_name(package: &BinaryPackageSpec, findings: &mut Vec<Finding>) {
    if package.name.ends_with("-dev") {
        findings.push(Finding {
            path: package.name.clone(),
            severity: Severity::Error,
            message: format!(
                "output '{}' uses '-dev'; development outputs must use '-devel'",
                package.name
            ),
        });
    }
    if package.name.ends_with("-common") {
        findings.push(Finding {
            path: package.name.clone(),
            severity: Severity::Error,
            message: format!(
                "output '{}' uses '-common'; split shared data by function or keep it in the owner",
                package.name
            ),
        });
    }
}

fn check_output_files(package: &BinaryPackageSpec, findings: &mut Vec<Finding>) {
    if package.name.ends_with("-devel") {
        for file in &package.files {
            if is_libdir_file(&file.path)
                && file.path.ends_with(".a")
                && !allows_static_archives(package)
            {
                findings.push(Finding {
                    path: package.name.clone(),
                    severity: Severity::Error,
                    message: format!(
                        "development output '{}' ships static archive '{}'; move it to a -static output",
                        package.name, file.path
                    ),
                });
            }
        }
    }
    if package.name.ends_with("-static") {
        for file in &package.files {
            if file.path.starts_with("/usr/include/")
                || (is_libdir_file(&file.path) && file.path.ends_with(".so"))
            {
                findings.push(Finding {
                    path: package.name.clone(),
                    severity: Severity::Error,
                    message: format!(
                        "static output '{}' ships non-static development file '{}'",
                        package.name, file.path
                    ),
                });
            }
        }
    }
}

fn check_output_dependencies(
    package: &BinaryPackageSpec,
    packages: &BTreeMap<&str, &BinaryPackageSpec>,
    findings: &mut Vec<Finding>,
) {
    let requires = requirement_names(package);

    if let Some(base) = package.name.strip_suffix("-libs") {
        if requires.contains(base) {
            findings.push(Finding {
                path: package.name.clone(),
                severity: Severity::Error,
                message: format!(
                    "runtime library output '{}' must not depend on command package '{}'",
                    package.name, base
                ),
            });
        }
        if let Some(command) = packages.get(base) {
            if !requirement_names(command).contains(package.name.as_str()) {
                findings.push(Finding {
                    path: package.name.clone(),
                    severity: Severity::Error,
                    message: format!(
                        "command package '{}' must depend on runtime library output '{}'",
                        base, package.name
                    ),
                });
            }
        }
    }

    if let Some(base) = package.name.strip_suffix("-devel") {
        let libs = format!("{base}-libs");
        if packages.contains_key(libs.as_str()) && !requires.contains(libs.as_str()) {
            findings.push(Finding {
                path: package.name.clone(),
                severity: Severity::Error,
                message: format!(
                    "development output '{}' must depend on runtime library output '{}'",
                    package.name, libs
                ),
            });
        }
    }

    if let Some(base) = package.name.strip_suffix("-static") {
        let devel = format!("{base}-devel");
        if !requires.contains(devel.as_str()) {
            findings.push(Finding {
                path: package.name.clone(),
                severity: Severity::Error,
                message: format!(
                    "static output '{}' must depend on development output '{}'",
                    package.name, devel
                ),
            });
        }
    }
}

fn requirement_names(package: &BinaryPackageSpec) -> BTreeSet<&str> {
    package
        .dependencies
        .requires
        .iter()
        .map(|require| require.split_whitespace().next().unwrap_or(require))
        .collect()
}

type FileCategory = (&'static str, &'static str, &'static str, fn(&str) -> bool);

fn check_main_package(package: &BinaryPackageSpec, findings: &mut Vec<Finding>) {
    let static_libraries: Vec<&str> = package
        .files
        .iter()
        .filter(|file| is_libdir_file(&file.path) && file.path.ends_with(".a"))
        .map(|file| file.path.as_str())
        .collect();
    if !static_libraries.is_empty() && !allows_static_archives(package) {
        findings.push(Finding {
            path: package.name.clone(),
            severity: Severity::Error,
            message: format!(
                "main package '{}' ships static archives (for example {}); move them to a -static output",
                package.name,
                static_libraries[..static_libraries.len().min(3)].join(", ")
            ),
        });
    }

    let categories: &[FileCategory] = &[
        (
            "development header",
            "development headers",
            "use files.include: [\"@headers\"] or [\"@dev\"]",
            |path| path.starts_with("/usr/include/"),
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

fn allows_static_archives(package: &BinaryPackageSpec) -> bool {
    package
        .lint
        .allow
        .iter()
        .any(|allow| allow == "static-archive")
}

/// The per-path rules, exposed for tests.
pub fn check_file(path: &str, mode: u32, license: bool, symlink: bool) -> Vec<Finding> {
    let mut findings = Vec::new();
    if let Some(helper) = path.strip_prefix("/usr/libexec/") {
        if !helper.contains('/') {
            findings.push(Finding {
                path: path.to_string(),
                severity: Severity::Error,
                message: format!("private helper {path} must live under /usr/libexec/<component>/"),
            });
        }
    }
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
                rpmx_base::output::error(format!("lint: {} [RPMX-POLICY-001]", finding.message))
            }
            Severity::Warning => {
                rpmx_base::output::warning("lint", format!("{} [RPMX-POLICY-001]", finding.message))
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
    use crate::package_spec::{BinaryPackageFile, BinaryPackageSpec};

    fn package(name: &str, files: &[&str], requires: &[&str]) -> BinaryPackageSpec {
        BinaryPackageSpec {
            name: name.to_string(),
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
            files: files
                .iter()
                .map(|path| BinaryPackageFile {
                    config: false,
                    noreplace: false,
                    doc: false,
                    license: path.starts_with("/usr/share/licenses/"),
                    path: (*path).to_string(),
                    data: Vec::new(),
                    mode: 0o644,
                    target: None,
                    user: None,
                    group: None,
                    caps: None,
                })
                .collect(),
            directories: Vec::new(),
            dependencies: rpmx_language::model::DependencySet {
                requires: requires.iter().map(|value| (*value).to_string()).collect(),
                ..Default::default()
            },
            lint: Default::default(),
            scriptlets: Default::default(),
        }
    }

    fn errors(findings: &[super::Finding]) -> Vec<&str> {
        findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .map(|finding| finding.message.as_str())
            .collect()
    }

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
            dependencies: rpmx_language::model::DependencySet {
                requires: vec!["bundled(zlib)".to_string()],
                ..Default::default()
            },
            lint: Default::default(),
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
            dependencies: rpmx_language::model::DependencySet::default(),
            lint: Default::default(),
            scriptlets: Default::default(),
        };
        let mut findings = Vec::new();
        check_main_package(&package, &mut findings);
        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Warning && finding.message.contains("@headers")
        }));
    }

    #[test]
    fn a_main_package_static_archive_is_an_error() {
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
            dependencies: rpmx_language::model::DependencySet::default(),
            lint: Default::default(),
            scriptlets: Default::default(),
        };
        let mut findings = Vec::new();
        check_main_package(&package, &mut findings);
        assert!(findings.iter().any(|finding| {
            finding.severity == Severity::Error && finding.message.contains("-static")
        }));
    }

    #[test]
    fn output_naming_and_split_rules_are_errors() {
        let findings = check(&[
            package("demo", &[], &["demo-libs"]),
            package("demo-libs", &["/usr/lib64/libdemo.so.1"], &[]),
            package(
                "demo-devel",
                &["/usr/include/demo.h", "/usr/lib64/libdemo.a"],
                &["demo-libs"],
            ),
            package("demo-dev", &[], &[]),
            package("demo-common", &[], &[]),
            package("demo-static", &["/usr/include/demo.h"], &["demo-devel"]),
        ]);
        let messages = errors(&findings);
        assert!(
            messages.iter().any(|message| message.contains("-devel")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("static archive")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|message| message.contains("-common")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("non-static")),
            "{messages:?}"
        );
    }

    #[test]
    fn runtime_library_dependency_direction_is_checked() {
        let findings = check(&[
            package("demo", &[], &[]),
            package("demo-libs", &["/usr/lib64/libdemo.so.1"], &["demo"]),
            package("demo-devel", &["/usr/include/demo.h"], &[]),
        ]);
        let messages = errors(&findings);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("must not depend")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("must depend on runtime")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|message| message.contains(
                "command package 'demo' must depend on runtime library output 'demo-libs'"
            )),
            "{messages:?}"
        );
    }

    #[test]
    fn a_command_package_that_depends_on_its_runtime_library_is_accepted() {
        let findings = check(&[
            package("demo", &[], &["demo-libs = 1.0-1"]),
            package("demo-libs", &["/usr/lib64/libdemo.so.1"], &[]),
            package("demo-devel", &["/usr/include/demo.h"], &["demo-libs"]),
        ]);
        assert!(errors(&findings).is_empty(), "{findings:?}");
    }

    #[test]
    fn a_recipe_can_allow_static_archives() {
        let mut main = package("gcc-16", &["/usr/lib64/libatomic.a"], &[]);
        main.lint.allow.push("static-archive".to_string());
        let mut development = package(
            "binutils-devel",
            &["/usr/lib64/bfd-plugins/libdep.a"],
            &["binutils"],
        );
        development.lint.allow.push("static-archive".to_string());
        let findings = check(&[main, package("binutils", &[], &[]), development]);
        assert!(errors(&findings).is_empty(), "{findings:?}");
    }

    #[test]
    fn private_helpers_need_a_component_directory() {
        assert!(check_file("/usr/libexec/frcode", 0o755, false, false)
            .iter()
            .any(|finding| finding.severity == Severity::Error));
        assert!(check_file("/usr/libexec/findutils/frcode", 0o755, false, false).is_empty());
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
