use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rpmx_base::error::{codes, Error, Result};
use rpmx_language::plan::{Plan, PlannedCell, PlannedOutput};
use serde::{Deserialize, Serialize};

use crate::naming::{
    cargo_version, go_version, node_version, plain_version, purl, python_version, ruby_version,
    valid_plain_name,
};
use crate::{INVENTORY_FILE, SCHEMA};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Golang,
    Cargo,
    Npm,
    Python,
    Ruby,
    Php,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Declared,
    Vendored,
    Embedded,
    Installed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Exact,
    Derived,
    Heuristic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Auto,
    Recipe,
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub path: String,
    pub cataloger: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub ecosystem: Ecosystem,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    pub scope: Scope,
    pub confidence: Confidence,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub outputs: BTreeSet<String>,
}

impl Component {
    pub(crate) fn new(
        ecosystem: Ecosystem,
        name: impl Into<String>,
        version: Option<String>,
        source: Source,
        scope: Scope,
        outputs: impl IntoIterator<Item = String>,
    ) -> Self {
        let name = name.into();
        let purl = version
            .as_deref()
            .map(|version| purl(ecosystem, &name, version));
        Self {
            ecosystem,
            name,
            version,
            purl,
            license: None,
            scope,
            confidence: Confidence::Exact,
            source,
            evidence: Vec::new(),
            outputs: outputs.into_iter().collect(),
        }
    }

    pub(crate) fn with_evidence(mut self, cataloger: &str, path: &str) -> Self {
        self.evidence.push(Evidence {
            path: path.to_string(),
            cataloger: cataloger.to_string(),
        });
        self
    }

    fn should_provide(&self) -> bool {
        self.scope != Scope::Declared && self.confidence != Confidence::Heuristic
    }

    /// The RPM provide string for this component, or `None` when it is only
    /// declared and not actually vendored.
    pub fn provide(&self) -> Option<String> {
        if !self.should_provide() {
            return None;
        }
        let name = match self.ecosystem {
            Ecosystem::Golang => format!("golang({})", self.name),
            Ecosystem::Cargo => format!("crate({})", self.name),
            Ecosystem::Npm => format!("nodejs-{}", self.name),
            Ecosystem::Python => format!("python3dist({})", self.name),
            Ecosystem::Ruby => format!("rubygem-{}", self.name),
            Ecosystem::Php => format!("php-composer({})", self.name),
            Ecosystem::Generic => self.name.clone(),
        };
        if matches!(
            self.ecosystem,
            Ecosystem::Npm
                | Ecosystem::Python
                | Ecosystem::Ruby
                | Ecosystem::Php
                | Ecosystem::Generic
        ) && !valid_plain_name(&self.name)
        {
            return None;
        }
        let version = self
            .version
            .as_deref()
            .map(|version| match self.ecosystem {
                Ecosystem::Golang => go_version(version),
                Ecosystem::Cargo => cargo_version(version),
                Ecosystem::Python => python_version(version),
                Ecosystem::Ruby => ruby_version(version),
                Ecosystem::Npm => node_version(version),
                Ecosystem::Php | Ecosystem::Generic => plain_version(version),
            })
            .filter(|version| !version.is_empty());
        Some(match version {
            Some(version) => format!("bundled({name}) = {version}"),
            None => format!("bundled({name})"),
        })
    }
}

fn scope_rank(scope: Scope) -> u8 {
    match scope {
        Scope::Declared => 0,
        Scope::Installed => 1,
        Scope::Vendored => 2,
        Scope::Embedded => 3,
    }
}

fn confidence_rank(confidence: Confidence) -> u8 {
    match confidence {
        Confidence::Heuristic => 0,
        Confidence::Derived => 1,
        Confidence::Exact => 2,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogerRef {
    pub name: String,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub schema: u32,
    #[serde(default)]
    pub catalogers: Vec<CatalogerRef>,
    #[serde(default)]
    pub components: Vec<Component>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            catalogers: Vec::new(),
            components: Vec::new(),
        }
    }
}

impl Inventory {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_plan(plan: &Plan) -> Self {
        Self::from_cells(&plan.cells.iter().collect::<Vec<_>>())
    }

    pub fn from_cells(cells: &[&PlannedCell]) -> Self {
        let mut inventory = Self::default();
        for cell in cells {
            for output in &cell.outputs {
                for declaration in &output.bundled {
                    let mut component = Component::new(
                        Ecosystem::Generic,
                        declaration.name.clone(),
                        declaration.version.clone(),
                        Source::Recipe,
                        Scope::Vendored,
                        [output.name.clone()],
                    )
                    .with_evidence("manual/recipe", "package.yaml");
                    component.license = declaration.license.clone();
                    inventory.components.push(component);
                }
            }
        }
        inventory
    }

    pub fn merge(&mut self, other: Inventory) -> Result<()> {
        self.catalogers.extend(other.catalogers);
        self.components.extend(other.components);
        self.normalize()
    }

    pub fn normalize(&mut self) -> Result<()> {
        self.schema = SCHEMA;
        self.catalogers
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.catalogers
            .dedup_by(|left, right| left.name == right.name && left.version == right.version);

        let mut merged: BTreeMap<(Vec<String>, Ecosystem, String, Option<String>), Component> =
            BTreeMap::new();
        for mut component in std::mem::take(&mut self.components) {
            component.evidence.sort_by(|left, right| {
                left.path
                    .cmp(&right.path)
                    .then_with(|| left.cataloger.cmp(&right.cataloger))
            });
            component.evidence.dedup();
            let key = (
                component.outputs.iter().cloned().collect(),
                component.ecosystem,
                component.name.clone(),
                component.version.clone(),
            );
            if let Some(previous) = merged.get_mut(&key) {
                if scope_rank(component.scope) > scope_rank(previous.scope) {
                    previous.scope = component.scope;
                }
                if confidence_rank(component.confidence) > confidence_rank(previous.confidence) {
                    previous.confidence = component.confidence;
                }
                previous.evidence.extend(component.evidence);
                previous.evidence.sort_by(|left, right| {
                    left.path
                        .cmp(&right.path)
                        .then_with(|| left.cataloger.cmp(&right.cataloger))
                });
                previous.evidence.dedup();
                if previous.license.is_none() {
                    previous.license = component.license;
                }
                if previous.purl.is_none() {
                    previous.purl = component.purl;
                }
            } else {
                merged.insert(key, component);
            }
        }
        self.components = merged.into_values().collect();
        self.validate_conflicts()
    }

    fn validate_conflicts(&self) -> Result<()> {
        for component in &self.components {
            if component.ecosystem != Ecosystem::Generic {
                continue;
            }
            let conflicting = self.components.iter().find(|other| {
                other.ecosystem == Ecosystem::Generic
                    && other.name == component.name
                    && other.outputs == component.outputs
                    && other.version != component.version
            });
            if let Some(other) = conflicting {
                return Err(Error::new(
                    codes::BUNDLED_002,
                    format!(
                        "output {} declares bundled component '{}' with both {:?} and {:?}",
                        component
                            .outputs
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(","),
                        component.name,
                        component.version,
                        other.version
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn components_for<'a>(
        &'a self,
        output: &'a PlannedOutput,
    ) -> impl Iterator<Item = &'a Component> + 'a {
        self.components
            .iter()
            .filter(move |component| component.outputs.contains(&output.name))
    }

    /// Provides for one output. The caller decides whether the generator is
    /// enabled; this only filters evidence and formats names.
    pub fn provides_for(&self, output: &PlannedOutput) -> Vec<String> {
        let mut provides = self
            .components
            .iter()
            .filter(|component| component.outputs.contains(&output.name))
            .filter_map(Component::provide)
            .collect::<Vec<_>>();
        provides.sort();
        provides.dedup();
        provides
    }

    pub fn provide_count(&self) -> usize {
        self.components
            .iter()
            .filter(|component| component.provide().is_some())
            .count()
    }

    /// Warn once per evidence file when a manifest only declares components
    /// but no vendor tree was found. A lockfile alone is not bundling evidence.
    pub fn warn_declared(&self) {
        let mut by_evidence: BTreeMap<&str, usize> = BTreeMap::new();
        for component in self
            .components
            .iter()
            .filter(|component| component.scope == Scope::Declared)
        {
            for evidence in &component.evidence {
                *by_evidence.entry(evidence.path.as_str()).or_default() += 1;
            }
        }
        for (path, count) in by_evidence {
            rpmx_base::output::warning(
                "bundled",
                format!(
                    "{path} declares {count} component(s) but no vendor tree was found; \
                     no bundled provides were generated"
                ),
            );
        }
    }

    pub fn to_json(&self) -> Result<String> {
        let mut inventory = self.clone();
        inventory.normalize()?;
        let mut text = serde_json::to_string_pretty(&inventory)
            .map_err(|error| Error::new(codes::BUNDLED_001, error.to_string()))?;
        text.push('\n');
        Ok(text)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let inventory: Inventory = serde_json::from_str(text).map_err(|error| {
            Error::new(
                codes::BUNDLED_001,
                format!("invalid {INVENTORY_FILE}: {error}"),
            )
        })?;
        if inventory.schema != SCHEMA {
            return Err(Error::new(
                codes::BUNDLED_001,
                format!(
                    "unsupported {INVENTORY_FILE} schema {}; this rpmx understands {SCHEMA}",
                    inventory.schema
                ),
            ));
        }
        Ok(inventory)
    }

    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            Error::new(
                codes::BUNDLED_001,
                format!("cannot read {}: {error}", path.display()),
            )
        })?;
        Self::from_json(&text)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let text = self.to_json()?;
        std::fs::write(path, text).map_err(|error| {
            Error::new(
                codes::BUNDLED_001,
                format!("cannot write {}: {error}", path.display()),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stronger_evidence_replaces_a_declared_only_component() {
        let mut declared = Inventory::empty();
        declared.components.push(
            Component::new(
                Ecosystem::Cargo,
                "serde",
                Some("1.0.203".to_string()),
                Source::Auto,
                Scope::Declared,
                ["app".to_string()],
            )
            .with_evidence("cargo/lock", "Cargo.lock"),
        );
        let mut vendored = Inventory::empty();
        vendored.components.push(
            Component::new(
                Ecosystem::Cargo,
                "serde",
                Some("1.0.203".to_string()),
                Source::Auto,
                Scope::Vendored,
                ["app".to_string()],
            )
            .with_evidence("cargo/lock", "Cargo.lock"),
        );
        declared.merge(vendored).expect("merge");
        assert_eq!(declared.components[0].scope, Scope::Vendored);
        assert_eq!(
            declared.components[0].provide().as_deref(),
            Some("bundled(crate(serde)) = 1.0.203")
        );
    }

    #[test]
    fn generic_components_keep_multiple_versions_out_of_one_output() {
        let mut inventory = Inventory::empty();
        for version in ["1.0", "2.0"] {
            inventory.components.push(
                Component::new(
                    Ecosystem::Generic,
                    "zlib",
                    Some(version.to_string()),
                    Source::Build,
                    Scope::Vendored,
                    ["app".to_string()],
                )
                .with_evidence("manual/build", "zlib.list"),
            );
        }
        assert_eq!(inventory.normalize().unwrap_err().code, codes::BUNDLED_002);
    }
}
