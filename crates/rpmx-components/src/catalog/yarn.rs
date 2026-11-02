//! Yarn classic and Berry `yarn.lock` cataloger.

use std::collections::BTreeSet;
use std::path::Path;

use rpmx_base::error::{codes, Error, Result};
use serde_yaml_ng::Value;

use crate::discover::{read_text, Cataloger, DiscoveryContext};
use crate::model::{Component, Ecosystem, Scope, Source};

pub(crate) struct YarnLock;

impl Cataloger for YarnLock {
    fn name(&self) -> &'static str {
        "node/yarn-lock"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        let mut components = Vec::new();
        for file in context.files() {
            if file.file_name().and_then(|name| name.to_str()) != Some("yarn.lock") {
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
            if installed(root) {
                parsed.retain(|component| installed_component(root, component));
            }
            components.extend(parsed);
        }
        Ok(components)
    }
}

fn installed(root: &Path) -> bool {
    root.join("node_modules").is_dir()
        || root.join(".yarn/cache").is_dir()
        || root.join(".pnp.cjs").is_file()
        || root.join(".pnp.js").is_file()
}

fn installed_component(root: &Path, component: &Component) -> bool {
    if manifest_matches(
        &root
            .join("node_modules")
            .join(&component.name)
            .join("package.json"),
        component,
    ) {
        return true;
    }
    if nested_manifest_matches(&root.join("node_modules"), component, 0) {
        return true;
    }
    let (Some(version), true) = (&component.version, root.join(".yarn/cache").is_dir()) else {
        return false;
    };
    let prefix = format!("{}-npm-{version}-", component.name.replace('/', "-"));
    std::fs::read_dir(root.join(".yarn/cache"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(hash) = name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".zip"))
            else {
                return false;
            };
            hash.len() >= 6 && hash.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}

fn manifest_matches(path: &Path, component: &Component) -> bool {
    if !path.exists() {
        return false;
    }
    let Ok(text) = read_text(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value.get("name").and_then(serde_json::Value::as_str) == Some(component.name.as_str())
        && value.get("version").and_then(serde_json::Value::as_str) == component.version.as_deref()
}

fn nested_manifest_matches(directory: &Path, component: &Component, depth: u8) -> bool {
    if depth > 4 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if entry.file_name() == "node_modules"
            && manifest_matches(&path.join(&component.name).join("package.json"), component)
        {
            return true;
        }
        if nested_manifest_matches(&path, component, depth + 1) {
            return true;
        }
    }
    false
}

fn parse(
    text: &str,
    evidence: &str,
    scope: Scope,
    outputs: &BTreeSet<String>,
) -> Result<Vec<Component>> {
    if text.lines().any(|line| line.trim() == "__metadata:") {
        parse_berry(text, evidence, scope, outputs)
    } else {
        parse_classic(text, evidence, scope, outputs)
    }
}

fn parse_classic(
    text: &str,
    evidence: &str,
    scope: Scope,
    outputs: &BTreeSet<String>,
) -> Result<Vec<Component>> {
    let mut components = Vec::new();
    let mut selector: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) && trimmed.ends_with(':') {
            let current = trimmed.trim_end_matches(':');
            selector = if current.contains("@file:")
                || current.contains("@link:")
                || current.contains("@portal:")
                || current.contains("@workspace:")
            {
                None
            } else {
                Some(current.to_string())
            };
            continue;
        }
        let Some(current) = &selector else {
            continue;
        };
        let Some(version) = trimmed.strip_prefix("version ") else {
            continue;
        };
        let Some(name) = classic_name(current) else {
            continue;
        };
        let version = version.trim().trim_matches('"').to_string();
        components.push(
            Component::new(
                Ecosystem::Npm,
                name,
                Some(version),
                Source::Auto,
                scope,
                outputs.iter().cloned(),
            )
            .with_evidence("node/yarn-lock", evidence),
        );
        selector = None;
    }
    Ok(components)
}

fn parse_berry(
    text: &str,
    evidence: &str,
    scope: Scope,
    outputs: &BTreeSet<String>,
) -> Result<Vec<Component>> {
    let value: Value = serde_yaml_ng::from_str(text).map_err(|error| {
        Error::new(
            codes::BUNDLED_001,
            format!("invalid yarn.lock {evidence}: {error}"),
        )
    })?;
    let Some(entries) = value.as_mapping() else {
        return Err(Error::new(
            codes::BUNDLED_001,
            format!("yarn.lock {evidence} is not a mapping"),
        ));
    };
    let mut components = Vec::new();
    for (selector, entry) in entries {
        let Some(selector) = selector.as_str() else {
            continue;
        };
        if selector == "__metadata" {
            continue;
        }
        let Some(entry) = entry.as_mapping() else {
            continue;
        };
        let version = entry.get(Value::String("version".to_string()));
        let Some(version) = version.and_then(Value::as_str) else {
            continue;
        };
        let link_type = entry
            .get(Value::String("linkType".to_string()))
            .and_then(Value::as_str);
        let resolution = entry
            .get(Value::String("resolution".to_string()))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if link_type == Some("soft")
            || resolution.contains("@workspace:")
            || version.starts_with("0.0.0-use.local")
        {
            continue;
        }
        let Some(name) = resolution_name(resolution).or_else(|| selector_name(selector)) else {
            continue;
        };
        components.push(
            Component::new(
                Ecosystem::Npm,
                name,
                Some(version.to_string()),
                Source::Auto,
                scope,
                outputs.iter().cloned(),
            )
            .with_evidence("node/yarn-lock", evidence),
        );
    }
    Ok(components)
}

fn classic_name(selector: &str) -> Option<String> {
    if let Some((_, target)) = selector.split_once("@npm:") {
        let target = target.trim().trim_matches('"');
        let start = usize::from(target.starts_with('@'));
        return Some(match target[start..].find('@') {
            Some(at) => target[..start + at].to_string(),
            None => target.to_string(),
        });
    }
    selector_name(selector)
}

fn resolution_name(resolution: &str) -> Option<String> {
    let name = resolution.split_once("@npm:").map(|(name, _)| name)?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn selector_name(selector: &str) -> Option<String> {
    let first = selector.split(',').next()?.trim().trim_matches('"');
    let start = usize::from(first.starts_with('@'));
    let at = first[start..].find('@')? + start;
    if at == 0 {
        return None;
    }
    Some(first[..at].to_string())
}

#[cfg(test)]
mod tests {
    use super::selector_name;

    #[test]
    fn selector_names_handle_scopes_and_protocols() {
        assert_eq!(
            selector_name("left-pad@^1.3.0").as_deref(),
            Some("left-pad")
        );
        assert_eq!(
            selector_name(r#""@babel/code-frame@npm:^7.0.0""#).as_deref(),
            Some("@babel/code-frame")
        );
        assert_eq!(
            selector_name("foo@^1.0.0, foo@^1.2.0").as_deref(),
            Some("foo")
        );
        assert_eq!(
            super::resolution_name("left-pad@npm:1.3.0").as_deref(),
            Some("left-pad")
        );
        assert_eq!(
            super::classic_name("left-pad-alias@npm:left-pad@^1.3.0").as_deref(),
            Some("left-pad")
        );
    }
}
