//! Exact build inputs and optional, output-scoped component coverage.
//!
//! This module does not infer which compiler or language a package uses.
//! Reports are assertions by trusted build steps, not observations by rpmx.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::package_spec::{BinaryPackageFile, BinaryPackageSpec};
use rpmx_base::error::{codes, Error, Result};
use serde::{Deserialize, Serialize};

pub const DIRECTORY: &str = ".rpmx/provenance";
pub const PAYLOAD_DIRECTORY: &str = "/usr/share/rpmx/buildinfo";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Role in the build: build or sysroot. Part of the report identity.
    pub root: String,
    pub name: String,
    pub evr: String,
    pub arch: String,
    pub checksum: String,
    pub source: Option<String>,
    /// Portable rpmx(*) provider capabilities. Unversioned values are empty.
    pub provides: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    pub schema: u32,
    pub output: String,
    pub root: String,
    pub checksum: String,
    /// Complete coverage of this provider's rpmx(component:*) capabilities.
    /// Toolchain and other capabilities cannot be excluded by this assertion.
    pub complete: bool,
    pub used: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema: u32,
    pub output: String,
    pub source: String,
    pub arch: String,
    /// True only for locked build and target roots; excludes source assembly.
    pub roots_complete: bool,
    pub inputs: Vec<Input>,
    pub coverage: Vec<Coverage>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(codes::BUILD_006, message.into())
}

pub struct Collected {
    pub inputs: Vec<Input>,
    pub complete: bool,
}

/// Project portable capabilities from the same inventory used for validation.
pub fn collect(inventory: &rpmx_root::root::VerifiedInputs, root: &str) -> Result<Collected> {
    let mut inputs = Vec::new();
    for input in inventory.packages() {
        let package = input.locked();
        let mut provides = BTreeMap::new();
        if let Some(header) = input.metadata() {
            for capability in &header.provides {
                if !capability.name.starts_with("rpmx(") {
                    continue;
                }
                if !capability.version.is_empty()
                    && capability.flags.intersection(
                        rpm::DependencyFlags::LESS
                            | rpm::DependencyFlags::GREATER
                            | rpm::DependencyFlags::EQUAL,
                    ) != rpm::DependencyFlags::EQUAL
                {
                    return Err(invalid(
                        "portable provider capabilities must have exact versions",
                    ));
                }
                if let Some(previous) =
                    provides.insert(capability.name.clone(), capability.version.clone())
                {
                    if previous != capability.version {
                        return Err(invalid("ambiguous provider capability versions"));
                    }
                }
            }
        }
        inputs.push(Input {
            root: root.into(),
            name: package.name.clone(),
            evr: package.evr.clone(),
            arch: input.arch().into(),
            checksum: package.checksum.clone(),
            source: package.srpm.clone(),
            provides,
        });
    }
    inputs.sort_by(|a, b| (&a.root, &a.name, &a.checksum).cmp(&(&b.root, &b.name, &b.checksum)));
    Ok(Collected {
        inputs,
        complete: inventory.complete(),
    })
}

/// Remove source-carried reports before the first writable build step.
pub fn prepare(build: &Path, inputs: &[Input]) -> Result<()> {
    let parent = build.join(".rpmx");
    if parent
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(invalid(".rpmx must not be a symlink"));
    }
    let directory = build.join(DIRECTORY);
    if directory.exists() || directory.symlink_metadata().is_ok() {
        if directory
            .symlink_metadata()
            .map_err(|e| invalid(e.to_string()))?
            .file_type()
            .is_symlink()
        {
            return Err(invalid("provenance directory must not be a symlink"));
        }
        std::fs::remove_dir_all(&directory).map_err(|e| invalid(e.to_string()))?;
    }
    std::fs::create_dir_all(directory.join("reports")).map_err(|e| invalid(e.to_string()))?;
    std::fs::write(
        directory.join("inputs.json"),
        serde_json::to_vec_pretty(inputs).map_err(|e| invalid(e.to_string()))?,
    )
    .map_err(|e| invalid(e.to_string()))
}

/// Snapshot reports before running tests. No report may name another output,
/// nonexistent provider, or capability that the provider did not advertise.
pub fn reports(
    build: &Path,
    inputs: &[Input],
    outputs: &BTreeSet<String>,
) -> Result<Vec<Coverage>> {
    for directory in [build.join(".rpmx"), build.join(DIRECTORY)] {
        if !directory
            .symlink_metadata()
            .map_err(|e| invalid(e.to_string()))?
            .file_type()
            .is_dir()
        {
            return Err(invalid("provenance ancestors must be real directories"));
        }
    }
    let directory = build.join(DIRECTORY).join("reports");
    if std::fs::symlink_metadata(&directory)
        .map_err(|e| invalid(e.to_string()))?
        .file_type()
        .is_symlink()
    {
        return Err(invalid("provenance reports must not be a symlink"));
    }
    let mut reports = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|e| invalid(e.to_string()))? {
        let entry = entry.map_err(|e| invalid(e.to_string()))?;
        if !entry
            .file_type()
            .map_err(|e| invalid(e.to_string()))?
            .is_file()
        {
            return Err(invalid("provenance reports must be regular JSON files"));
        }
        let report: Coverage = serde_json::from_slice(
            &std::fs::read(entry.path()).map_err(|e| invalid(e.to_string()))?,
        )
        .map_err(|e| invalid(format!("invalid component coverage: {e}")))?;
        validate_report(&report, inputs, outputs)?;
        if reports.iter().any(|r: &Coverage| {
            r.output == report.output && r.root == report.root && r.checksum == report.checksum
        }) {
            return Err(invalid("duplicate provider coverage for an output"));
        }
        reports.push(report);
    }
    reports
        .sort_by(|a, b| (&a.output, &a.root, &a.checksum).cmp(&(&b.output, &b.root, &b.checksum)));
    Ok(reports)
}

fn validate_report(report: &Coverage, inputs: &[Input], outputs: &BTreeSet<String>) -> Result<()> {
    if report.schema != 1 || !outputs.contains(&report.output) {
        return Err(invalid("unknown coverage schema or output"));
    }
    let provider = inputs
        .iter()
        .find(|p| p.root == report.root && p.checksum == report.checksum)
        .ok_or_else(|| invalid("coverage provider is not in the resolved build root"))?;
    if report
        .used
        .iter()
        .any(|c| !c.starts_with("rpmx(component:") || !provider.provides.contains_key(c))
    {
        return Err(invalid(
            "coverage names a component not advertised by its provider",
        ));
    }
    Ok(())
}

impl Receipt {
    fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(invalid("unknown provenance schema"));
        }
        let outputs = BTreeSet::from([self.output.clone()]);
        let mut seen = BTreeSet::new();
        for report in &self.coverage {
            validate_report(report, &self.inputs, &outputs)?;
            if !seen.insert((&report.root, &report.checksum)) {
                return Err(invalid("duplicate component coverage"));
            }
        }
        Ok(())
    }

    /// Potential influence, refined only by complete component coverage.
    pub fn capabilities(&self) -> BTreeSet<(String, String)> {
        let mut result = BTreeSet::new();
        for input in &self.inputs {
            let report = self
                .coverage
                .iter()
                .find(|r| r.root == input.root && r.checksum == input.checksum);
            for (name, version) in &input.provides {
                let Some(inner) = name.strip_prefix("rpmx(").and_then(|v| v.strip_suffix(')'))
                else {
                    continue;
                };
                if inner.starts_with("component:")
                    && report.is_some_and(|r| r.complete && !r.used.contains(name))
                {
                    continue;
                }
                result.insert((format!("buildinput({inner})"), version.clone()));
            }
        }
        result
    }
}

pub fn attach(
    package: &mut BinaryPackageSpec,
    inputs: &[Input],
    reports: &[Coverage],
    complete: bool,
) -> Result<()> {
    let receipt = Receipt {
        schema: 1,
        output: package.name.clone(),
        source: package.source_rpm.clone(),
        arch: package.arch.clone(),
        roots_complete: complete,
        inputs: inputs.into(),
        coverage: reports
            .iter()
            .filter(|r| r.output == package.name)
            .cloned()
            .collect(),
    };
    receipt.validate()?;
    for (name, value) in receipt.capabilities() {
        package.dependencies.provides.push(if value.is_empty() {
            name
        } else {
            format!("{name} = {value}")
        });
    }
    let path = format!("{PAYLOAD_DIRECTORY}/{}.json", package.name);
    if package.files.iter().any(|file| file.path == path) {
        return Err(invalid(
            "package payload already contains the reserved provenance receipt",
        ));
    }
    package.files.push(BinaryPackageFile {
        path,
        data: serde_json::to_vec_pretty(&receipt).map_err(|e| invalid(e.to_string()))?,
        mode: 0o644,
        target: None,
        user: None,
        group: None,
        caps: None,
        config: false,
        noreplace: false,
        doc: false,
        license: false,
    });
    Ok(())
}

/// Legacy RPMs return None. Malformed receipts fail rather than becoming legacy.
pub fn inspect(path: &Path) -> Result<Option<Receipt>> {
    let package = rpm::Package::open(path).map_err(|e| invalid(e.to_string()))?;
    let name = package
        .metadata
        .get_name()
        .map_err(|e| invalid(e.to_string()))?;
    let expected = format!("{PAYLOAD_DIRECTORY}/{name}.json");
    let files = package
        .metadata
        .get_file_paths()
        .map_err(|e| invalid(e.to_string()))?;
    if !files.iter().any(|p| p.to_string_lossy() == expected) {
        return Ok(None);
    }
    let temporary = tempfile::tempdir().map_err(|e| invalid(e.to_string()))?;
    rpmx_payload::extract_payload(&package, temporary.path())?;
    let record = temporary.path().join(expected.trim_start_matches('/'));
    if !record
        .symlink_metadata()
        .map_err(|e| invalid(e.to_string()))?
        .file_type()
        .is_file()
    {
        return Err(invalid("provenance receipt is not a regular file"));
    }
    let receipt: Receipt =
        serde_json::from_slice(&std::fs::read(record).map_err(|e| invalid(e.to_string()))?)
            .map_err(|e| invalid(e.to_string()))?;
    if receipt.schema != 1
        || receipt.output != name
        || receipt.arch
            != package
                .metadata
                .get_arch()
                .map_err(|e| invalid(e.to_string()))?
        || receipt.source
            != package
                .metadata
                .get_source_rpm()
                .map_err(|e| invalid(e.to_string()))?
    {
        return Err(invalid("provenance identity differs from RPM header"));
    }
    receipt.validate()?;
    Ok(Some(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provenance_and_requirements_share_verified_metadata_and_coverage() {
        use rpmx_root::root::{closure_digest, RequirementStatus, RootLock, VerifiedInputs};
        let cache = tempfile::tempdir().unwrap();
        let mut builder = rpm::PackageBuilder::new("provider", "2", "MIT", "noarch", "fixture");
        builder
            .release("1")
            .provides(rpm::Dependency::eq("gcc-c++", "16"))
            .provides(rpm::Dependency::eq("rpmx(component:fixture)", "2"));
        let path = cache.path().join("provider.rpm");
        builder.build().unwrap().write_file(&path).unwrap();
        let checksum = rpmx_store::cas::store(cache.path(), &std::fs::read(path).unwrap()).unwrap();
        let mut lock: RootLock = serde_json::from_value(serde_json::json!({
            "document": "rpmx-root-lock", "version": 2, "profile": "fixture",
            "archs": ["x86_64"], "releasever": "44", "repositories": [],
            "packages": {"noarch": [{"name": "provider", "evr": "2-1", "checksum": checksum,
                "origin": {"kind": "candidate"}, "size": 0}]}, "closure_digest": ""
        }))
        .unwrap();
        lock.closure_digest = closure_digest(
            &lock.releasever,
            &lock.archs,
            &lock.repositories,
            &lock.packages,
        );
        let inventory = VerifiedInputs::load(&lock, cache.path()).unwrap();
        // Both consumers use the snapshot even if the cache is removed later.
        std::fs::remove_file(rpmx_store::cas::path(cache.path(), &checksum).unwrap()).unwrap();
        assert_eq!(
            inventory.requirement("gcc-c++ >= 16").unwrap(),
            RequirementStatus::Satisfied
        );
        let collected = collect(&inventory, "build").unwrap();
        assert!(collected.complete);
        assert_eq!(
            collected.inputs[0].provides,
            BTreeMap::from([("rpmx(component:fixture)".into(), "2".into())])
        );
        assert_eq!(collected.inputs[0].checksum, checksum);
        let incomplete = VerifiedInputs::load(&lock, cache.path()).unwrap();
        assert_eq!(
            incomplete.requirement("gcc-c++").unwrap(),
            RequirementStatus::Unknown
        );
        let collected = collect(&incomplete, "build").unwrap();
        assert!(!collected.complete);
        assert!(collected.inputs[0].provides.is_empty());
        assert_eq!(collected.inputs[0].name, "provider");
    }

    fn input() -> Input {
        Input {
            root: "build".into(),
            name: "compiler".into(),
            evr: "1-3".into(),
            arch: "x86_64".into(),
            checksum: "sha256:123".into(),
            source: Some("compiler-1-3.src.rpm".into()),
            provides: BTreeMap::from([
                ("rpmx(component:crypto)".into(), "2".into()),
                ("rpmx(component:core)".into(), "1".into()),
                ("rpmx(toolchain:example)".into(), "1".into()),
            ]),
        }
    }
    fn report(complete: bool) -> Coverage {
        Coverage {
            schema: 1,
            output: "app".into(),
            root: "build".into(),
            checksum: "sha256:123".into(),
            complete,
            used: BTreeSet::from(["rpmx(component:core)".into()]),
        }
    }
    #[test]
    fn only_complete_coverage_excludes_unused_components() {
        let mut receipt = Receipt {
            schema: 1,
            output: "app".into(),
            source: "app-1-1.src.rpm".into(),
            arch: "x86_64".into(),
            roots_complete: true,
            inputs: vec![input()],
            coverage: vec![],
        };
        assert_eq!(receipt.capabilities().len(), 3);
        receipt.coverage.push(report(false));
        assert_eq!(receipt.capabilities().len(), 3);
        receipt.coverage[0].complete = true;
        assert_eq!(
            receipt.capabilities(),
            BTreeSet::from([
                ("buildinput(component:core)".into(), "1".into()),
                ("buildinput(toolchain:example)".into(), "1".into())
            ])
        );
    }
    #[test]
    fn reports_cannot_invent_inputs_components_or_outputs() {
        let outputs = BTreeSet::from(["app".into()]);
        assert!(validate_report(&report(true), &[input()], &outputs).is_ok());
        for bad in [
            Coverage {
                checksum: "other".into(),
                ..report(true)
            },
            Coverage {
                output: "other".into(),
                ..report(true)
            },
            Coverage {
                used: BTreeSet::from(["rpmx(component:unknown)".into()]),
                ..report(true)
            },
            Coverage {
                schema: 2,
                ..report(true)
            },
        ] {
            assert!(validate_report(&bad, &[input()], &outputs).is_err());
        }
    }
    #[test]
    fn source_carried_reports_are_erased_and_duplicate_reports_fail() {
        let build = tempfile::tempdir().unwrap();
        prepare(build.path(), &[input()]).unwrap();
        let directory = build.path().join(DIRECTORY).join("reports");
        std::fs::write(directory.join("old.json"), "untrusted source data").unwrap();
        prepare(build.path(), &[input()]).unwrap();
        assert!(
            reports(build.path(), &[input()], &BTreeSet::from(["app".into()]))
                .unwrap()
                .is_empty()
        );
        for name in ["a.json", "b.json"] {
            std::fs::write(
                directory.join(name),
                serde_json::to_vec(&report(true)).unwrap(),
            )
            .unwrap();
        }
        assert!(reports(build.path(), &[input()], &BTreeSet::from(["app".into()])).is_err());
    }
}
