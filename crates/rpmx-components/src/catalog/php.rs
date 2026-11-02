//! PHP Composer lock and installed-package cataloger.

use rpmx_base::error::{codes, Error, Result};
use serde_json::Value;

use crate::discover::{read_text, Cataloger, DiscoveryContext};
use crate::model::{Component, Ecosystem, Scope, Source};

pub(crate) struct Composer;

impl Cataloger for Composer {
    fn name(&self) -> &'static str {
        "php/composer"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        let mut components = Vec::new();
        for file in context.files() {
            let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let evidence = context.relative(file);
            if name == "composer.lock" {
                // installed.json is the authoritative installed set when it
                // exists; the lock alone is only a declaration.
                let scope = if file.parent().is_some_and(|root| {
                    !root.join("vendor/composer/installed.json").is_file()
                        && root.join("vendor/autoload.php").is_file()
                }) {
                    Scope::Installed
                } else {
                    Scope::Declared
                };
                let text = read_text(file)?;
                for (name, version) in lock_packages(&text, &evidence)? {
                    components.push(component(
                        name,
                        version,
                        scope,
                        "php/composer-lock",
                        &evidence,
                        context,
                    ));
                }
            } else if name == "installed.json"
                && is_vendor_path(file)
                && file
                    .parent()
                    .and_then(std::path::Path::file_name)
                    .is_some_and(|name| name == "composer")
            {
                let text = read_text(file)?;
                let dev = file
                    .parent()
                    .and_then(|composer| composer.parent())
                    .and_then(|vendor| vendor.parent())
                    .map(|root| root.join("composer.lock"))
                    .filter(|lock| lock.is_file())
                    .map(|lock| lock_dev_names(&lock))
                    .transpose()?
                    .unwrap_or_default();
                for (name, version) in installed_packages(&text, &evidence, &dev)? {
                    components.push(component(
                        name,
                        version,
                        Scope::Installed,
                        "php/composer-installed",
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
    version: String,
    scope: Scope,
    cataloger: &str,
    evidence: &str,
    context: &DiscoveryContext<'_>,
) -> Component {
    Component::new(
        Ecosystem::Php,
        name,
        Some(version),
        Source::Auto,
        scope,
        context.auto_outputs().iter().cloned(),
    )
    .with_evidence(cataloger, evidence)
}

fn is_vendor_path(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| part == "vendor")
    })
}

fn lock_dev_names(path: &std::path::Path) -> Result<std::collections::BTreeSet<String>> {
    let text = crate::discover::read_text(path)?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        Error::new(
            codes::BUNDLED_001,
            format!("invalid composer.lock {}: {error}", path.display()),
        )
    })?;
    Ok(value
        .get("packages-dev")
        .and_then(Value::as_array)
        .map(|packages| {
            packages
                .iter()
                .filter_map(|package| package.get("name"))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}

fn lock_packages(text: &str, evidence: &str) -> Result<Vec<(String, String)>> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        Error::new(
            codes::BUNDLED_001,
            format!("invalid composer.lock {evidence}: {error}"),
        )
    })?;
    packages(value.get("packages"))
}

fn installed_packages(
    text: &str,
    evidence: &str,
    lock_dev: &std::collections::BTreeSet<String>,
) -> Result<Vec<(String, String)>> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        Error::new(
            codes::BUNDLED_001,
            format!("invalid installed.json {evidence}: {error}"),
        )
    })?;
    let mut dev = value
        .get("dev-package-names")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();
    dev.extend(lock_dev.iter().cloned());
    let packages = if value.is_array() {
        packages(Some(&value))?
    } else {
        packages(value.get("packages"))?
    };
    Ok(packages
        .into_iter()
        .filter(|(name, _)| !dev.contains(name.as_str()))
        .collect())
}

fn packages(value: Option<&Value>) -> Result<Vec<(String, String)>> {
    let Some(packages) = value.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for package in packages {
        if package.get("dev").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let Some(name) = package.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(version) = package.get("version").and_then(Value::as_str) else {
            continue;
        };
        if !name.contains('/') || version.is_empty() {
            continue;
        }
        result.push((name.to_string(), version.to_string()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::installed_packages;

    #[test]
    fn composer_dev_package_names_are_skipped() {
        let text = r#"{
            "packages": [
                {"name": "psr/container", "version": "1.1.1"},
                {"name": "phpunit/phpunit", "version": "11.0.0"},
                {"name": "invalid", "version": "1.0"}
            ],
            "dev-package-names": ["phpunit/phpunit"]
        }"#;
        assert_eq!(
            installed_packages(text, "installed.json", &Default::default()).expect("packages"),
            vec![("psr/container".to_string(), "1.1.1".to_string())]
        );
    }

    #[test]
    fn composer_v1_array_uses_lock_dev_names() {
        let text = r#"[
            {"name": "psr/container", "version": "1.1.1"},
            {"name": "phpunit/phpunit", "version": "11.0.0"}
        ]"#;
        let dev = ["phpunit/phpunit".to_string()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            installed_packages(text, "installed.json", &dev).expect("packages"),
            vec![("psr/container".to_string(), "1.1.1".to_string())]
        );
    }
}
