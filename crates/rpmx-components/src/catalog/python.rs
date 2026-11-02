//! Python `METADATA`, `PKG-INFO` and `vendored.txt` cataloger.

use std::path::Path;

use rpmx_base::error::Result;

use crate::discover::{read_text, Cataloger, DiscoveryContext};
use crate::model::{Component, Ecosystem, Scope, Source};

pub(crate) struct PythonDist;

impl Cataloger for PythonDist {
    fn name(&self) -> &'static str {
        "python/dist"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        let mut components = Vec::new();
        for file in context.files() {
            let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let evidence = context.relative(file);
            if name == "METADATA" || name == "PKG-INFO" {
                if !file.parent().is_some_and(is_dist_info) || !is_installed_path(file) {
                    continue;
                }
                let text = read_text(file)?;
                if let Some((name, version)) = metadata(&text) {
                    if context
                        .package_names()
                        .iter()
                        .any(|package| matches_package(package, &name))
                    {
                        continue;
                    }
                    components.push(component(
                        name,
                        version,
                        Scope::Installed,
                        "python/dist",
                        &evidence,
                        context,
                    ));
                }
            } else if name == "vendored.txt" {
                let scope = if is_installed_path(file) {
                    Scope::Vendored
                } else {
                    Scope::Declared
                };
                let text = read_text(file)?;
                for (name, version) in requirements(&text) {
                    components.push(component(
                        name,
                        version,
                        scope,
                        "python/vendored",
                        &evidence,
                        context,
                    ));
                }
            }
        }
        Ok(components)
    }
}

fn component(
    name: String,
    version: Option<String>,
    scope: Scope,
    cataloger: &str,
    evidence: &str,
    context: &DiscoveryContext<'_>,
) -> Component {
    Component::new(
        Ecosystem::Python,
        name,
        version,
        Source::Auto,
        scope,
        context.auto_outputs().iter().cloned(),
    )
    .with_evidence(cataloger, evidence)
}

fn matches_package(package: &str, dist: &str) -> bool {
    let package = normalize_name(package);
    if package == dist {
        return true;
    }
    ["python3-", "python-"].iter().any(|prefix| {
        package
            .strip_prefix(prefix)
            .is_some_and(|stripped| stripped == dist)
    })
}

fn is_installed_path(path: &Path) -> bool {
    !path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| matches!(part, ".venv" | "venv" | ".tox"))
    }) && path.components().any(|component| {
        component.as_os_str().to_str().is_some_and(|part| {
            matches!(
                part,
                "site-packages" | "dist-packages" | "vendor" | "_vendor" | "third_party"
            )
        })
    })
}

fn is_dist_info(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".dist-info") || name.ends_with(".egg-info"))
}

fn metadata(text: &str) -> Option<(String, Option<String>)> {
    let mut name = None;
    let mut version = None;
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Name:") {
            name = Some(normalize_name(value.trim()));
        } else if let Some(value) = line.strip_prefix("Version:") {
            version = Some(value.trim().to_string());
        }
    }
    name.map(|name| (name, version))
}

fn requirements(text: &str) -> Vec<(String, Option<String>)> {
    let mut components = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or(line).trim();
        if line.is_empty() {
            continue;
        }
        let line = line.split(';').next().unwrap_or(line).trim();
        let (name, version) = match line.split_once("==") {
            Some((name, version)) => {
                let version = version.trim();
                (
                    name.trim().split('[').next().unwrap_or(name).trim(),
                    if version.ends_with(".*") {
                        None
                    } else {
                        Some(version.to_string())
                    },
                )
            }
            None => (
                line.split(['<', '>', '=', '!', '~', ' '])
                    .next()
                    .unwrap_or(line)
                    .split('[')
                    .next()
                    .unwrap_or(line)
                    .trim(),
                None,
            ),
        };
        if !name.is_empty() {
            components.push((normalize_name(name), version));
        }
    }
    components
}

fn normalize_name(name: &str) -> String {
    let mut normalized = String::new();
    let mut separator = false;
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !separator && !normalized.is_empty() {
                normalized.push('-');
            }
            separator = true;
        } else {
            normalized.push(ch.to_ascii_lowercase());
            separator = false;
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::{metadata, normalize_name, requirements};

    #[test]
    fn names_are_normalized_like_pep_503() {
        assert_eq!(normalize_name("Foo.Bar_baz"), "foo-bar-baz");
        assert_eq!(normalize_name("zope.interface"), "zope-interface");
    }

    #[test]
    fn metadata_and_requirements_are_read() {
        assert_eq!(
            metadata("Metadata-Version: 2.1\nName: foo_bar\nVersion: 26.0\n"),
            Some(("foo-bar".to_string(), Some("26.0".to_string())))
        );
        assert_eq!(
            requirements("requests==2.32.5\nfoo>=1\nwild==1.2.*\n# comment\n"),
            vec![
                ("requests".to_string(), Some("2.32.5".to_string())),
                ("foo".to_string(), None),
                ("wild".to_string(), None)
            ]
        );
    }
}
