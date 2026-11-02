//! pnpm `pnpm-lock.yaml` cataloger.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rpmx_base::error::{codes, Error, Result};
use serde::Deserialize;
use serde_yaml_ng::Value;

use crate::discover::{read_text, Cataloger, DiscoveryContext};
use crate::model::{Component, Ecosystem, Scope, Source};

pub(crate) struct PnpmLock;

impl Cataloger for PnpmLock {
    fn name(&self) -> &'static str {
        "node/pnpm-lock"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        let mut components = Vec::new();
        for file in context.files() {
            if file.file_name().and_then(|name| name.to_str()) != Some("pnpm-lock.yaml") {
                continue;
            }
            let Some(root) = file.parent() else {
                continue;
            };
            let scope = if installed(root) {
                Scope::Installed
            } else {
                Scope::Declared
            };
            let text = read_text(file)?;
            let evidence = context.relative(file);
            let mut parsed = parse(&text, &evidence, scope, context.auto_outputs())?;
            if root.join("node_modules").is_dir() {
                parsed.retain(|component| installed_package(root, component));
            }
            components.extend(parsed);
        }
        Ok(components)
    }
}

fn installed(root: &Path) -> bool {
    root.join("node_modules").is_dir() || root.join("node_modules/.pnpm").is_dir()
}

fn installed_package(root: &Path, component: &Component) -> bool {
    let manifest = root
        .join("node_modules")
        .join(&component.name)
        .join("package.json");
    if manifest.exists() {
        if let Ok(text) = crate::discover::read_text(&manifest) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if value.get("name").and_then(serde_json::Value::as_str)
                    == Some(component.name.as_str())
                    && value.get("version").and_then(serde_json::Value::as_str)
                        == component.version.as_deref()
                {
                    return true;
                }
            }
        }
    }
    let Some(version) = &component.version else {
        return false;
    };
    let encoded = component.name.replace('/', "+");
    let prefix = format!("{encoded}@{version}");
    let store = root.join("node_modules/.pnpm");
    let Ok(entries) = std::fs::read_dir(&store) else {
        return false;
    };
    entries.filter_map(std::result::Result::ok).any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name == prefix
            || name.starts_with(&format!("{prefix}_"))
            || name.starts_with(&format!("{prefix}("))
    })
}

fn parse(
    text: &str,
    evidence: &str,
    scope: Scope,
    outputs: &BTreeSet<String>,
) -> Result<Vec<Component>> {
    let mapping = documents(text, evidence)?;
    let packages = mapping
        .get(Value::String("packages".to_string()))
        .and_then(Value::as_mapping);
    let snapshots = mapping
        .get(Value::String("snapshots".to_string()))
        .and_then(Value::as_mapping);
    // pnpm 9 puts identity in packages and reachability/dev markers in
    // snapshots. Older lockfiles only have packages.
    let entries: Box<dyn Iterator<Item = (&Value, &Value)>> = match (snapshots, packages) {
        (Some(snapshots), _) => Box::new(snapshots.iter()),
        (None, Some(packages)) => Box::new(packages.iter()),
        (None, None) => Box::new(std::iter::empty()),
    };

    let mut parsed = Vec::new();
    for (package, entry) in entries {
        let identity = entry_name_version(entry)
            .or_else(|| {
                packages
                    .and_then(|packages| packages.get(package))
                    .and_then(entry_name_version)
            })
            .or_else(|| package.as_str().and_then(package_key));
        let Some((name, version)) = identity else {
            continue;
        };
        parsed.push(ParsedEntry {
            name,
            version,
            dev: is_true(entry, "dev"),
            dependencies: dependency_names(entry),
        });
    }

    let production = production_names(&mapping);
    let selected = match production {
        Some(production) => reachable(&parsed, &production),
        None => parsed.iter().map(|_| true).collect::<Vec<_>>(),
    };

    let mut components = Vec::new();
    for (entry, selected) in parsed.into_iter().zip(selected) {
        if entry.dev || !selected {
            continue;
        }
        components.push(
            Component::new(
                Ecosystem::Npm,
                entry.name,
                Some(entry.version),
                Source::Auto,
                scope,
                outputs.iter().cloned(),
            )
            .with_evidence("node/pnpm-lock", evidence),
        );
    }
    Ok(components)
}

struct ParsedEntry {
    name: String,
    version: String,
    dev: bool,
    dependencies: BTreeSet<String>,
}

fn is_true(entry: &Value, key: &str) -> bool {
    entry
        .as_mapping()
        .and_then(|entry| entry.get(Value::String(key.to_string())))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn dependency_names(entry: &Value) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Some(entry) = entry.as_mapping() else {
        return names;
    };
    for key in ["dependencies", "optionalDependencies"] {
        let Some(dependencies) = entry
            .get(Value::String(key.to_string()))
            .and_then(Value::as_mapping)
        else {
            continue;
        };
        for (name, _) in dependencies {
            if let Some(name) = name.as_str() {
                names.insert(name.to_string());
            }
        }
    }
    names
}

fn production_names(mapping: &serde_yaml_ng::Mapping) -> Option<BTreeSet<String>> {
    let root = mapping
        .get(Value::String("importers".to_string()))
        .and_then(Value::as_mapping)
        .and_then(|importers| importers.get(Value::String(".".to_string())))
        .and_then(Value::as_mapping)
        .or_else(|| {
            mapping
                .get(Value::String("dependencies".to_string()))
                .and_then(Value::as_mapping)
                .map(|_| mapping)
        })?;
    let mut names = BTreeSet::new();
    for key in ["dependencies", "optionalDependencies"] {
        let Some(dependencies) = root
            .get(Value::String(key.to_string()))
            .and_then(Value::as_mapping)
        else {
            continue;
        };
        for (name, _) in dependencies {
            if let Some(name) = name.as_str() {
                names.insert(name.to_string());
            }
        }
    }
    Some(names)
}

fn reachable(entries: &[ParsedEntry], production: &BTreeSet<String>) -> Vec<bool> {
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        by_name.entry(entry.name.as_str()).or_default().push(index);
    }
    let mut selected = vec![false; entries.len()];
    let mut pending = production.iter().cloned().collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    while let Some(name) = pending.pop_first() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(indexes) = by_name.get(name.as_str()) else {
            continue;
        };
        for index in indexes {
            if selected[*index] {
                continue;
            }
            selected[*index] = true;
            pending.extend(entries[*index].dependencies.iter().cloned());
        }
    }
    selected
}

/// pnpm 11/12 can write a second YAML document for the environment lockfile.
/// Merge the top-level mappings in document order.
fn documents(text: &str, evidence: &str) -> Result<serde_yaml_ng::Mapping> {
    let mut merged = serde_yaml_ng::Mapping::new();
    let mut seen = false;
    for document in serde_yaml_ng::Deserializer::from_str(text) {
        let value = Value::deserialize(document).map_err(|error| {
            Error::new(
                codes::BUNDLED_001,
                format!("invalid pnpm-lock.yaml {evidence}: {error}"),
            )
        })?;
        if let Some(mapping) = value.as_mapping() {
            merged.extend(mapping.clone());
            seen = true;
        }
    }
    if !seen {
        return Err(Error::new(
            codes::BUNDLED_001,
            format!("pnpm-lock.yaml {evidence} is empty"),
        ));
    }
    Ok(merged)
}

fn entry_name_version(entry: &Value) -> Option<(String, String)> {
    let entry = entry.as_mapping()?;
    let name = entry
        .get(Value::String("name".to_string()))?
        .as_str()?
        .to_string();
    let version = entry.get(Value::String("version".to_string()))?.as_str()?;
    let version = clean_version(version)?;
    Some((name, version))
}

fn package_key(key: &str) -> Option<(String, String)> {
    let key = key.trim();
    if key.is_empty()
        || key.starts_with("link:")
        || key.starts_with("file:")
        || key.contains("://")
        || key.contains("@file:")
        || key.contains("@link:")
    {
        return None;
    }
    let key = key.split_once('(').map_or(key, |(key, _)| key);

    if let Some(rest) = key.strip_prefix('/') {
        if let Some((name, candidate)) = rest.rsplit_once('/') {
            if looks_like_version(candidate) {
                return Some((name.to_string(), clean_version(candidate)?));
            }
        }
        return split_name_version(rest);
    }
    split_name_version(key)
}

fn split_name_version(key: &str) -> Option<(String, String)> {
    let (name, version) = key.rsplit_once('@')?;
    if name.is_empty() || name == "@" || version.is_empty() {
        return None;
    }
    if name.contains('/') && !name.starts_with('@') {
        return None;
    }
    Some((name.to_string(), clean_version(version)?))
}

fn looks_like_version(value: &str) -> bool {
    let value = value.trim_start_matches('v');
    value.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

fn clean_version(version: &str) -> Option<String> {
    let version = version
        .split_once('(')
        .map_or(version, |(version, _)| version);
    let version = version
        .split_once('_')
        .map_or(version, |(version, _)| version);
    if version.is_empty() {
        None
    } else {
        // Keep a syntactically odd version so the naming layer degrades it to
        // an unversioned provide instead of dropping the component.
        Some(version.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::package_key;

    #[test]
    fn package_keys_cover_v5_v6_and_v9() {
        assert_eq!(
            package_key("/left-pad/1.3.0"),
            Some(("left-pad".to_string(), "1.3.0".to_string()))
        );
        assert_eq!(
            package_key("/@scope/name/1.2.3"),
            Some(("@scope/name".to_string(), "1.2.3".to_string()))
        );
        assert_eq!(
            package_key("/left-pad@1.3.0"),
            Some(("left-pad".to_string(), "1.3.0".to_string()))
        );
        assert_eq!(
            package_key("/@scope/name@1.2.3"),
            Some(("@scope/name".to_string(), "1.2.3".to_string()))
        );
        assert_eq!(
            package_key("left-pad@1.3.0"),
            Some(("left-pad".to_string(), "1.3.0".to_string()))
        );
        assert_eq!(
            package_key("@scope/name@1.2.3(peer@2.0.0)"),
            Some(("@scope/name".to_string(), "1.2.3".to_string()))
        );
        assert_eq!(
            package_key("/@scope/name/1.2.3_peer@2.0.0"),
            Some(("@scope/name".to_string(), "1.2.3".to_string()))
        );
        assert_eq!(
            package_key("is-negative@https://codeload.github.com/x/tar.gz/abc"),
            None
        );
        assert_eq!(package_key("registry.example.com/foo@1.0.0"), None);
    }
}
