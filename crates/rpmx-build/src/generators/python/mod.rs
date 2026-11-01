//! `python.attr` and `pythondist.attr`: interpreter ABI and distribution
//! dependencies.
//!
//! Only the version-specific `pythonX.Ydist(...)` names are generated.
//! A distro that wants a major-only alias for its default interpreter can
//! declare it on the runtime package; rpmx does not assume one.

mod marker;
mod version;

use std::collections::{BTreeMap, BTreeSet};

use crate::package_spec::BinaryPackageFile;

use super::Findings;

#[derive(Default)]
pub struct Scanner {
    findings: Findings,
    abi_provides: BTreeSet<String>,
}

impl Scanner {
    pub fn inspect(&mut self, file: &BinaryPackageFile, target_arch: &str) {
        if let Some(provide) = abi_provide(&file.path) {
            self.abi_provides.insert(provide.clone());
            self.findings.provide(provide);
        }
        if let Some(require) = abi_require(&file.path) {
            self.findings.require(require);
        }
        if let Some(distribution) = Distribution::from_file(file) {
            self.distribution(distribution, target_arch);
        }
    }

    pub fn finish(mut self) -> Findings {
        self.findings
            .retain_requires(|require| !self.abi_provides.contains(require));
        self.findings
    }

    fn distribution(&mut self, distribution: Distribution, target_arch: &str) {
        let namespace = format!("python{}dist", distribution.python_version);
        let name = format!("{namespace}({})", distribution.name);
        match &distribution.version {
            Some(value) => self
                .findings
                .provide(format!("{name} = {}", version::to_rpm(value))),
            None => self.findings.provide(name),
        }

        let abi = format!("python(abi) = {}", distribution.python_version);
        if !self.abi_provides.contains(&abi) {
            self.findings.require(abi);
        }

        let environment = marker::Environment {
            python_version: &distribution.python_version,
            platform_machine: target_arch,
            extra: "",
        };
        for requirement in &distribution.requires {
            let Some(requirement) = Requirement::parse(requirement) else {
                continue;
            };
            let include = match &requirement.marker {
                None => true,
                Some(marker) => match marker::evaluate(marker, &environment) {
                    marker::Tristate::True => true,
                    marker::Tristate::False => false,
                    // An unmodelled marker is more likely to be a real
                    // dependency than a phantom one, except for extras.
                    marker::Tristate::Unknown => !marker.contains("extra"),
                },
            };
            if !include {
                continue;
            }
            let dependency = format!("{namespace}({})", requirement.name);
            let constraints = version::requirements(&dependency, &requirement.specifiers);
            if constraints.is_empty() {
                self.findings.require(dependency);
            } else {
                for constraint in constraints {
                    self.findings.require(constraint);
                }
            }
        }
    }
}

fn abi_provide(path: &str) -> Option<String> {
    let version = path.strip_prefix("/usr/bin/python")?;
    is_version(version).then(|| format!("python(abi) = {version}"))
}

fn abi_require(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix("/usr/lib64")
        .or_else(|| path.strip_prefix("/usr/lib"))?;
    let rest = rest.strip_prefix("/python")?;
    let (version, tail) = rest.split_once('/')?;
    if !is_version(version)
        || !(tail.ends_with(".py")
            || tail.ends_with(".pyc")
            || tail.ends_with(".pyo")
            || tail.ends_with(".so"))
    {
        return None;
    }
    Some(format!("python(abi) = {version}"))
}

fn is_version(value: &str) -> bool {
    let Some((major, minor)) = value.split_once('.') else {
        return false;
    };
    !major.is_empty()
        && !minor.is_empty()
        && major.chars().all(|character| character.is_ascii_digit())
        && minor.chars().all(|character| character.is_ascii_digit())
}

struct Distribution {
    python_version: String,
    name: String,
    version: Option<String>,
    requires: Vec<String>,
}

impl Distribution {
    fn from_file(file: &BinaryPackageFile) -> Option<Self> {
        let (python_version, tail) = site_packages(&file.path)?;
        if !(tail.ends_with(".dist-info/METADATA")
            || tail.ends_with(".egg-info/PKG-INFO")
            || tail.ends_with(".egg-info"))
        {
            return None;
        }
        let text = std::str::from_utf8(&file.data).ok()?;
        let headers = parse_headers(text);
        let name = headers.get("Name")?.first()?.clone();
        let name = normalize_name(&name);
        if name.is_empty() {
            return None;
        }
        Some(Self {
            python_version: python_version.to_string(),
            name,
            version: headers
                .get("Version")
                .and_then(|values| values.first())
                .filter(|version| !version.trim().is_empty())
                .cloned(),
            requires: headers.get("Requires-Dist").cloned().unwrap_or_default(),
        })
    }
}

fn site_packages(path: &str) -> Option<(&str, &str)> {
    let rest = path
        .strip_prefix("/usr/lib64/python")
        .or_else(|| path.strip_prefix("/usr/lib/python"))?;
    let (version, tail) = rest.split_once('/')?;
    if !is_version(version) {
        return None;
    }
    let tail = tail.strip_prefix("site-packages/")?;
    Some((version, tail))
}

fn parse_headers(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(key) = &current {
                if let Some(value) = headers.get_mut(key).and_then(|values| values.last_mut()) {
                    value.push(' ');
                    value.push_str(line.trim());
                }
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            current = None;
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            current = None;
            continue;
        }
        headers
            .entry(key.to_string())
            .or_default()
            .push(value.trim().to_string());
        current = Some(key.to_string());
    }
    headers
}

/// PEP 503 normalization: lower case, and every run of `-`, `_`, `.` becomes
/// a single hyphen.
fn normalize_name(name: &str) -> String {
    let mut normalized = String::new();
    let mut separator = false;
    for character in name.chars() {
        if matches!(character, '-' | '_' | '.') {
            if !separator && !normalized.is_empty() {
                normalized.push('-');
            }
            separator = true;
        } else {
            normalized.push(character.to_ascii_lowercase());
            separator = false;
        }
    }
    normalized.trim_end_matches('-').to_string()
}

struct Requirement {
    name: String,
    specifiers: String,
    marker: Option<String>,
}

impl Requirement {
    fn parse(value: &str) -> Option<Self> {
        let (requirement, marker) = match value.split_once(';') {
            Some((requirement, marker)) => (requirement, Some(marker.trim().to_string())),
            None => (value, None),
        };
        let requirement = requirement.trim();
        let end = requirement
            .find(['<', '>', '=', '!', '~', '[', ' ', '\t'])
            .unwrap_or(requirement.len());
        let name = normalize_name(requirement[..end].trim());
        if name.is_empty() {
            return None;
        }
        let rest = requirement[end..].trim();
        let specifiers = match rest.find('[') {
            Some(start) => match rest[start..].find(']') {
                Some(end) => rest[start + end + 1..].trim().to_string(),
                None => String::new(),
            },
            None => rest.to_string(),
        };
        Some(Self {
            name,
            specifiers,
            marker,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::test_file;

    fn inspect(path: &str, data: &str) -> (Vec<String>, Vec<String>) {
        let mut scanner = Scanner::default();
        scanner.inspect(&test_file(path, data.as_bytes()), "x86_64");
        scanner.finish().into_dependencies()
    }

    #[test]
    fn the_interpreter_provides_its_abi() {
        let (provides, _) = inspect("/usr/bin/python3.12", "");
        assert_eq!(provides, vec!["python(abi) = 3.12"]);
    }

    #[test]
    fn installed_python_files_require_the_abi() {
        let (_, requires) = inspect("/usr/lib64/python3.12/site-packages/foo/__init__.py", "");
        assert_eq!(requires, vec!["python(abi) = 3.12"]);
    }

    #[test]
    fn dist_metadata_provides_the_versioned_distribution_name() {
        let (provides, _) = inspect(
            "/usr/lib64/python3.12/site-packages/foo-2.0.0.dist-info/METADATA",
            "Metadata-Version: 2.1\nName: Foo_Bar\nVersion: 2.0.0\n",
        );
        assert!(provides.contains(&"python3.12dist(foo-bar) = 2".to_string()));
    }

    #[test]
    fn dist_requires_are_evaluated_and_versioned() {
        let (_, requires) = inspect(
            "/usr/lib64/python3.12/site-packages/foo-1.0.dist-info/METADATA",
            "Name: foo\nVersion: 1.0\n\
             Requires-Dist: bar>=2.0\n\
             Requires-Dist: oldpkg; python_version < \"3.8\"\n\
             Requires-Dist: testpkg; extra == \"test\"\n\
             Requires-Dist: winpkg; sys_platform == \"win32\"\n",
        );
        assert!(requires.contains(&"python3.12dist(bar) >= 2".to_string()));
        assert!(!requires.iter().any(|require| require.contains("oldpkg")));
        assert!(!requires.iter().any(|require| require.contains("testpkg")));
        assert!(!requires.iter().any(|require| require.contains("winpkg")));
    }

    #[test]
    fn dist_metadata_requires_the_abi_when_the_package_does_not_ship_it() {
        let (_, requires) = inspect(
            "/usr/lib64/python3.12/site-packages/foo-1.0.dist-info/METADATA",
            "Name: foo\nVersion: 1.0\n",
        );
        assert!(requires.contains(&"python(abi) = 3.12".to_string()));
    }

    #[test]
    fn extras_on_a_requirement_still_require_the_base_distribution() {
        let (_, requires) = inspect(
            "/usr/lib/python3.12/site-packages/foo-1.0.dist-info/METADATA",
            "Name: foo\nVersion: 1.0\nRequires-Dist: bar[extra1]>=1.0\n",
        );
        assert!(requires.contains(&"python3.12dist(bar) >= 1".to_string()));
    }

    #[test]
    fn a_package_does_not_require_the_abi_it_ships() {
        let mut scanner = Scanner::default();
        scanner.inspect(&test_file("/usr/bin/python3.12", b""), "x86_64");
        scanner.inspect(
            &test_file("/usr/lib64/python3.12/site-packages/foo/__init__.py", b""),
            "x86_64",
        );
        assert!(scanner.finish().into_dependencies().1.is_empty());
    }
}
