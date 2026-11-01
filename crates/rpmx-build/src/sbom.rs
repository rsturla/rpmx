use std::path::Path;

use serde_json::{json, Value};

use rpmx_base::error::{codes, Error, Result};
use rpmx_base::hash::sha512_hex;
use rpmx_root::root::RootLock;

pub const SBOM_DIR: &str = "/usr/share/sbom";

pub struct SourceRef<'a> {
    pub path: &'a str,
    pub digest: &'a str,
}

pub struct BinaryRef<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub release: &'a str,
    pub arch: &'a str,
    pub license: &'a str,
    pub path: &'a Path,
}

pub struct PackageSbom<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub release: &'a str,
    pub arch: &'a str,
    pub license: &'a str,
    pub source_rpm: &'a str,
    pub recipe_digest: &'a str,
    pub source_date_epoch: u32,
    pub sources: &'a [SourceRef<'a>],
    pub cpe: Option<&'a rpmx_language::model::Cpe>,
}

pub struct BuildSbom<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub release: &'a str,
    pub arch: &'a str,
    pub license: &'a str,
    pub recipe_digest: &'a str,
    pub source_date_epoch: u32,
    pub sources: &'a [SourceRef<'a>],
    pub binaries: &'a [BinaryRef<'a>],
    pub root_lock: Option<&'a RootLock>,
    /// Fingerprint of the OpenPGP key used to sign the binaries, if any.
    pub signing_key: Option<&'a str>,
    pub cpe: Option<&'a rpmx_language::model::Cpe>,
}

/// The SBOM backend. A Syft-based implementation can replace `NativeSpdx`
/// without changing callers.
pub trait Generator {
    fn package(&self, input: &PackageSbom<'_>) -> Result<String>;
    fn build(&self, input: &BuildSbom<'_>) -> Result<String>;
}

pub struct NativeSpdx;

pub fn native_spdx() -> NativeSpdx {
    NativeSpdx
}

impl Generator for NativeSpdx {
    fn package(&self, input: &PackageSbom<'_>) -> Result<String> {
        let package_id = format!("SPDXRef-Package-{}", id(input.name));
        let config_id = "SPDXRef-BuildConfig";
        let source_ids = input
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let id = format!("SPDXRef-Source-{index}");
                (id.clone(), spdx_file(&id, source.path, source.digest))
            })
            .collect::<Vec<_>>();

        let cpe = input.cpe.map(|cpe| cpe.cpe23(input.version, input.release));
        let mut packages = vec![spdx_package(
            &package_id,
            input.name,
            &full_version(input.version, input.release),
            input.license,
            purl(input.name, input.version, Some(input.release), input.arch),
            None,
            cpe.as_deref(),
        )];
        packages.push(spdx_package(
            "SPDXRef-SourceRPM",
            input.source_rpm,
            &full_version(input.version, input.release),
            input.license,
            purl(input.name, input.version, Some(input.release), "src"),
            None,
            cpe.as_deref(),
        ));
        packages.extend(source_ids.iter().map(|(_, package)| package.clone()));
        packages.push(spdx_file(config_id, "package.yaml", input.recipe_digest));

        let mut relationships = vec![
            relationship("SPDXRef-DOCUMENT", "DESCRIBES", &package_id),
            relationship(&package_id, "GENERATED_FROM", "SPDXRef-SourceRPM"),
            relationship(&package_id, "DESCRIBED_BY", config_id),
        ];
        for (source_id, _) in &source_ids {
            relationships.push(relationship(&package_id, "GENERATED_FROM", source_id));
        }

        document(
            format!("{}-{}-{}", input.name, input.version, input.release),
            format!(
                "https://rpmx.dev/spdx/{}-{}-{}-{}",
                input.name, input.version, input.release, input.arch
            ),
            input.source_date_epoch,
            packages,
            relationships,
        )
    }

    fn build(&self, input: &BuildSbom<'_>) -> Result<String> {
        let source_id = format!("SPDXRef-SourcePackage-{}", id(input.name));
        let config_id = "SPDXRef-BuildConfig";
        let source_cpe = input.cpe.map(|cpe| cpe.cpe23(input.version, input.release));
        let mut packages = vec![spdx_package(
            &source_id,
            input.name,
            &full_version(input.version, input.release),
            input.license,
            purl(input.name, input.version, Some(input.release), "src"),
            None,
            source_cpe.as_deref(),
        )];
        let mut relationships = vec![relationship("SPDXRef-DOCUMENT", "DESCRIBES", &source_id)];

        for source in input.sources {
            let source_ref = format!("SPDXRef-Source-{}", id(source.path));
            packages.push(spdx_file(&source_ref, source.path, source.digest));
            relationships.push(relationship(&source_id, "CONTAINS", &source_ref));
        }
        packages.push(spdx_file(config_id, "package.yaml", input.recipe_digest));
        relationships.push(relationship(&source_id, "DESCRIBED_BY", config_id));

        for binary in input.binaries {
            let binary_id = format!("SPDXRef-Binary-{}-{}", id(binary.name), id(binary.arch));
            let bytes = std::fs::read(binary.path).map_err(|error| {
                Error::new(
                    codes::SBOM_001,
                    format!("cannot read {}: {error}", binary.path.display()),
                )
            })?;
            let binary_cpe = input
                .cpe
                .map(|cpe| cpe.cpe23(binary.version, binary.release));
            let mut package = spdx_package(
                &binary_id,
                binary.name,
                &full_version(binary.version, binary.release),
                binary.license,
                purl(
                    binary.name,
                    binary.version,
                    Some(binary.release),
                    binary.arch,
                ),
                Some(&sha512_hex(&bytes)),
                binary_cpe.as_deref(),
            );
            if let Some(fingerprint) = input.signing_key {
                package["annotations"] = json!([{
                    "annotationType": "OTHER",
                    "annotator": format!("Tool: rpmx-{}", env!("CARGO_PKG_VERSION")),
                    "annotationDate": iso8601(input.source_date_epoch),
                    "comment": format!("OpenPGP signature by key {fingerprint}"),
                }]);
            }
            packages.push(package);
            relationships.push(relationship(&binary_id, "GENERATED_FROM", &source_id));
        }

        if let Some(lock) = input.root_lock {
            for (arch, entries) in &lock.packages {
                for package in entries {
                    let package_id = format!("SPDXRef-Root-{}-{}", id(&package.name), id(arch));
                    packages.push(spdx_package(
                        &package_id,
                        &package.name,
                        &package.evr,
                        "NOASSERTION",
                        purl(&package.name, &package.evr, None, arch),
                        Some(&package.checksum),
                        None,
                    ));
                    relationships.push(relationship(
                        &package_id,
                        "BUILD_DEPENDENCY_OF",
                        &source_id,
                    ));
                }
            }
        }

        document(
            format!("{}-{}-{}-build", input.name, input.version, input.release),
            format!(
                "https://rpmx.dev/spdx/{}-{}-{}-{}-build",
                input.name, input.version, input.release, input.arch
            ),
            input.source_date_epoch,
            packages,
            relationships,
        )
    }
}

fn document(
    name: String,
    namespace: String,
    source_date_epoch: u32,
    packages: Vec<Value>,
    relationships: Vec<Value>,
) -> Result<String> {
    let value = json!({
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": name,
        "documentNamespace": namespace,
        "creationInfo": {
            "created": iso8601(source_date_epoch),
            "creators": [format!("Tool: rpmx-{}", env!("CARGO_PKG_VERSION"))]
        },
        "packages": packages,
        "relationships": relationships
    });
    let mut text = serde_json::to_string_pretty(&value)
        .map_err(|error| Error::new(codes::SBOM_001, error.to_string()))?;
    text.push('\n');
    Ok(text)
}

fn spdx_package(
    id: &str,
    name: &str,
    version: &str,
    license: &str,
    purl: String,
    checksum: Option<&str>,
    cpe: Option<&str>,
) -> Value {
    let mut package = json!({
        "SPDXID": id,
        "name": name,
        "versionInfo": version,
        "downloadLocation": "NOASSERTION",
        "filesAnalyzed": false,
        "licenseConcluded": license,
        "licenseDeclared": license,
        "copyrightText": "NOASSERTION",
        "supplier": "NOASSERTION",
        "externalRefs": [{
            "referenceCategory": "PACKAGE-MANAGER",
            "referenceType": "purl",
            "referenceLocator": purl
        }]
    });
    if let Some(checksum) = checksum {
        package["checksums"] = json!([{ "algorithm": "SHA512", "checksumValue": hex(checksum) }]);
    }
    if let Some(cpe) = cpe {
        if let Some(refs) = package["externalRefs"].as_array_mut() {
            refs.push(json!({
                "referenceCategory": "SECURITY",
                "referenceType": "cpe23Type",
                "referenceLocator": cpe
            }));
        }
    }
    package
}

fn spdx_file(spdx_id: &str, name: &str, digest: &str) -> Value {
    json!({
        "SPDXID": spdx_id,
        "name": name,
        "downloadLocation": "NOASSERTION",
        "filesAnalyzed": false,
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": "NOASSERTION",
        "copyrightText": "NOASSERTION",
        "checksums": [{ "algorithm": "SHA512", "checksumValue": hex(digest) }]
    })
}

fn relationship(from: &str, kind: &str, to: &str) -> Value {
    json!({
        "spdxElementId": from,
        "relationshipType": kind,
        "relatedSpdxElement": to
    })
}

fn full_version(version: &str, release: &str) -> String {
    format!("{version}-{release}")
}

fn purl(name: &str, version: &str, release: Option<&str>, arch: &str) -> String {
    let version = match release {
        Some(release) => format!("{version}-{release}"),
        None => version.to_string(),
    };
    format!("pkg:rpm/{name}@{version}?arch={arch}")
}

fn hex(digest: &str) -> &str {
    digest.strip_prefix("sha512:").unwrap_or(digest)
}

fn id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn iso8601(epoch: u32) -> String {
    let days = i64::from(epoch) / 86_400;
    let seconds = i64::from(epoch) % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

// Howard Hinnant's civil_from_days, which is valid for the epoch range we use.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}
