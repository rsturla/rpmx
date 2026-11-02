//! npm `package-lock.json` and installed `node_modules` catalogers.

use std::collections::BTreeSet;
use std::path::Path;

use rpmx_base::error::{codes, Error, Result};
use serde_json::Value;

use crate::discover::{read_text, Cataloger, DiscoveryContext};
use crate::model::{Component, Ecosystem, Scope, Source};

pub(crate) struct NodeLock;

impl Cataloger for NodeLock {
    fn name(&self) -> &'static str {
        "node/package-lock"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        let mut components = Vec::new();
        for file in context.files() {
            if file.file_name().and_then(|name| name.to_str()) != Some("package-lock.json") {
                continue;
            }
            let root = file.parent();
            let installed = root.is_some_and(|parent| parent.join("node_modules").is_dir());
            let text = read_text(file)?;
            components.extend(parse_lock(
                &text,
                &context.relative(file),
                if installed {
                    Scope::Installed
                } else {
                    Scope::Declared
                },
                context.auto_outputs(),
                root,
            )?);
        }
        Ok(components)
    }
}

pub(crate) struct NodeInstalled;

impl Cataloger for NodeInstalled {
    fn name(&self) -> &'static str {
        "node/installed"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        let mut components = Vec::new();
        for directory in context.dirs().iter().filter(|directory| {
            directory
                .file_name()
                .is_some_and(|name| name == "node_modules")
        }) {
            if governed_by_lock(directory, context.root()) {
                continue;
            }
            let production = production_names_for_install(directory, context.root())?;
            components.extend(parse_installed(
                directory,
                &context.relative(directory),
                context.auto_outputs(),
                production.as_ref(),
            )?);
        }
        Ok(components)
    }
}

fn production_names_for_install(
    node_modules: &Path,
    root: &Path,
) -> Result<Option<std::collections::BTreeSet<String>>> {
    let Some(manifest) = governing_manifest(node_modules, root) else {
        return Ok(None);
    };
    let text = read_text(&manifest)?;
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let mut pending = std::collections::BTreeSet::new();
    for key in ["dependencies", "optionalDependencies"] {
        let Some(dependencies) = value.get(key).and_then(Value::as_object) else {
            continue;
        };
        pending.extend(dependencies.keys().cloned());
    }

    let installed = installed_dependencies(node_modules)?;
    let mut allowed = std::collections::BTreeSet::new();
    while let Some(name) = pending.pop_first() {
        if !allowed.insert(name.clone()) {
            continue;
        }
        if let Some(dependencies) = installed.get(&name) {
            pending.extend(dependencies.iter().cloned());
        }
    }
    Ok(Some(allowed))
}

fn governing_manifest(node_modules: &Path, root: &Path) -> Option<std::path::PathBuf> {
    let mut current = node_modules.parent();
    while let Some(directory) = current {
        let manifest = directory.join("package.json");
        if manifest.is_file() {
            return Some(manifest);
        }
        if directory == root {
            break;
        }
        current = directory.parent();
    }
    None
}

fn installed_dependencies(
    node_modules: &Path,
) -> Result<std::collections::BTreeMap<String, std::collections::BTreeSet<String>>> {
    let mut result = std::collections::BTreeMap::new();
    for entry in rpmx_base::io::sorted_entries(node_modules, codes::BUNDLED_003)? {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('@') {
            for scoped in rpmx_base::io::sorted_entries(&path, codes::BUNDLED_003)? {
                if scoped.file_type().is_ok_and(|kind| kind.is_dir()) {
                    read_dependencies(&scoped.path(), &mut result)?;
                }
            }
        } else if name != ".bin" {
            read_dependencies(&path, &mut result)?;
        }
    }
    Ok(result)
}

fn read_dependencies(
    directory: &Path,
    result: &mut std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) -> Result<()> {
    let manifest = directory.join("package.json");
    if !manifest.is_file() {
        return Ok(());
    }
    let text = read_text(&manifest)?;
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let Some(name) = value.get("name").and_then(Value::as_str) else {
        return Ok(());
    };
    let mut dependencies = std::collections::BTreeSet::new();
    for key in ["dependencies", "optionalDependencies"] {
        let Some(values) = value.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (dependency, _) in values {
            dependencies.insert(dependency.clone());
        }
    }
    result.insert(name.to_string(), dependencies);
    Ok(())
}

/// A lockfile is the production set. Once one is present, the lock-specific
/// cataloger owns provides; scanning the installed tree would also advertise
/// development dependencies.
fn governed_by_lock(node_modules: &Path, root: &Path) -> bool {
    let mut current = node_modules.parent();
    while let Some(directory) = current {
        if directory.join("package-lock.json").is_file()
            || directory.join("yarn.lock").is_file()
            || directory.join("pnpm-lock.yaml").is_file()
        {
            return true;
        }
        if directory == root {
            break;
        }
        current = directory.parent();
    }
    false
}

fn parse_lock(
    text: &str,
    evidence: &str,
    scope: Scope,
    outputs: &BTreeSet<String>,
    root: Option<&Path>,
) -> Result<Vec<Component>> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        Error::new(
            codes::BUNDLED_001,
            format!("invalid package-lock.json {evidence}: {error}"),
        )
    })?;
    let mut packages = Vec::new();

    if let Some(packages_value) = value.get("packages").and_then(Value::as_object) {
        for (path, package) in packages_value {
            if path.is_empty()
                || package
                    .get("link")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                || package.get("dev").and_then(Value::as_bool).unwrap_or(false)
            {
                continue;
            }
            let Some(name) = path.rsplit_once("node_modules/").map(|(_, name)| name) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let Some(version) = package.get("version").and_then(Value::as_str) else {
                continue;
            };
            if scope == Scope::Installed && root.is_some_and(|root| !root.join(path).is_dir()) {
                continue;
            }
            packages.push(
                Component::new(
                    Ecosystem::Npm,
                    name,
                    Some(version.to_string()),
                    Source::Auto,
                    scope,
                    outputs.iter().cloned(),
                )
                .with_evidence("node/package-lock", evidence),
            );
        }
    } else if let Some(dependencies) = value.get("dependencies").and_then(Value::as_object) {
        parse_v1(dependencies, evidence, scope, outputs, root, &mut packages);
    }

    Ok(packages)
}

fn parse_v1(
    dependencies: &serde_json::Map<String, Value>,
    evidence: &str,
    scope: Scope,
    outputs: &BTreeSet<String>,
    root: Option<&Path>,
    packages: &mut Vec<Component>,
) {
    for (name, package) in dependencies {
        if package.get("dev").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let installed = scope != Scope::Installed
            || root.is_none_or(|root| root.join("node_modules").join(name).is_dir());
        if installed {
            if let Some(version) = package.get("version").and_then(Value::as_str) {
                packages.push(
                    Component::new(
                        Ecosystem::Npm,
                        name,
                        Some(version.to_string()),
                        Source::Auto,
                        scope,
                        outputs.iter().cloned(),
                    )
                    .with_evidence("node/package-lock", evidence),
                );
            }
        }
        if let Some(nested) = package.get("dependencies").and_then(Value::as_object) {
            parse_v1(nested, evidence, scope, outputs, root, packages);
        }
    }
}

fn parse_installed(
    node_modules: &Path,
    evidence: &str,
    outputs: &BTreeSet<String>,
    allowed: Option<&BTreeSet<String>>,
) -> Result<Vec<Component>> {
    let mut components = Vec::new();
    for entry in rpmx_base::io::sorted_entries(node_modules, codes::BUNDLED_003)? {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || name == ".bin" {
            continue;
        }
        if name.starts_with('@') {
            for scoped in rpmx_base::io::sorted_entries(&path, codes::BUNDLED_003)? {
                if !scoped.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let scoped_name = format!("{}/{}", name, scoped.file_name().to_string_lossy());
                if allowed.is_some_and(|allowed| !allowed.contains(&scoped_name)) {
                    continue;
                }
                read_package(&scoped.path(), evidence, outputs, &mut components)?;
            }
        } else {
            if allowed.is_some_and(|allowed| !allowed.contains(name.as_ref())) {
                continue;
            }
            read_package(&path, evidence, outputs, &mut components)?;
        }
    }
    Ok(components)
}

fn read_package(
    directory: &Path,
    evidence: &str,
    outputs: &BTreeSet<String>,
    components: &mut Vec<Component>,
) -> Result<()> {
    let manifest = directory.join("package.json");
    if !manifest.exists() {
        return Ok(());
    }
    let text = read_text(&manifest)?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        Error::new(
            codes::BUNDLED_001,
            format!("invalid {}: {error}", manifest.display()),
        )
    })?;
    let Some(name) = value.get("name").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(version) = value.get("version").and_then(Value::as_str) else {
        return Ok(());
    };
    let relative = format!(
        "{evidence}/{}/package.json",
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("package")
    );
    components.push(
        Component::new(
            Ecosystem::Npm,
            name,
            Some(version.to_string()),
            Source::Auto,
            Scope::Installed,
            outputs.iter().cloned(),
        )
        .with_evidence("node/installed", &relative),
    );
    Ok(())
}
