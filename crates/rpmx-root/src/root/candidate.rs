//! Immutable candidate RPMs. Original input paths are never retained.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rpmx_base::error::{codes, Error, Result};
use rpmx_payload::metadata::{package_metadata, PackageMetadata};
use rpmx_store::cas;

#[derive(Clone, Debug)]
pub struct Candidate {
    metadata: PackageMetadata,
    checksum: String,
    size: u64,
}

impl Candidate {
    pub fn metadata(&self) -> &PackageMetadata {
        &self.metadata
    }
    pub fn checksum(&self) -> &str {
        &self.checksum
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn spec(&self) -> String {
        format!(
            "{}-{}.{}",
            self.metadata.name,
            self.metadata.evr(),
            self.metadata.arch
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct CandidateSet {
    packages: Vec<Candidate>,
}

impl CandidateSet {
    /// Snapshot bytes before inspecting headers. A pathname changing later is irrelevant.
    pub fn import(cache: &Path, paths: &[PathBuf]) -> Result<Self> {
        let mut packages: Vec<Candidate> = Vec::new();
        for path in paths {
            let bytes = std::fs::read(path).map_err(|error| {
                Error::new(
                    codes::ROOT_011,
                    format!("cannot import candidate {}: {error}", path.display()),
                )
            })?;
            let checksum = cas::store(cache, &bytes)?;
            // Also reject a pre-existing corrupted CAS object.
            cas::load(cache, &checksum)?;
            let snapshot = cas::path(cache, &checksum)?;
            let package = rpm::Package::open(&snapshot).map_err(|error| {
                Error::new(
                    codes::ROOT_010,
                    format!("cannot inspect candidate {checksum}: {error}"),
                )
            })?;
            package.verify_digests().map_err(|error| {
                Error::new(
                    codes::ROOT_010,
                    format!("candidate {checksum} has invalid RPM digests: {error}"),
                )
            })?;
            if package.metadata.is_source_package() {
                return Err(Error::new(
                    codes::ROOT_009,
                    "candidate inputs must be binary RPMs",
                ));
            }
            let metadata = package_metadata(&snapshot)?;
            if matches!(metadata.arch.as_str(), "src" | "nosrc") {
                return Err(Error::new(
                    codes::ROOT_009,
                    "candidate inputs must be binary RPMs",
                ));
            }
            if metadata.arch != "noarch" {
                rpmx_base::arch::Arch::parse(&metadata.arch)?;
            }
            if let Some(existing) = packages.iter().find(|candidate| {
                candidate.metadata.name == metadata.name && candidate.metadata.arch == metadata.arch
            }) {
                if existing.checksum != checksum {
                    return Err(Error::new(
                        codes::ROOT_009,
                        format!(
                            "multiple candidate payloads for {}.{}",
                            metadata.name, metadata.arch
                        ),
                    ));
                }
                continue;
            }
            packages.push(Candidate {
                metadata,
                checksum,
                size: bytes.len() as u64,
            });
        }
        packages.sort_by_key(Candidate::spec);
        Ok(Self { packages })
    }

    pub fn packages(&self) -> &[Candidate] {
        &self.packages
    }
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    pub fn select(&self, names: &[String]) -> Result<Self> {
        let wanted = names.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let packages = self
            .packages
            .iter()
            .filter(|candidate| wanted.contains(candidate.metadata.name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        for name in wanted {
            if !packages
                .iter()
                .any(|candidate| candidate.metadata.name == name)
            {
                return Err(Error::new(
                    codes::ROOT_009,
                    format!("candidate output '{name}' is absent"),
                ));
            }
        }
        Ok(Self { packages })
    }

    pub(crate) fn stage(&self, cache: &Path, directory: &Path) -> Result<Vec<String>> {
        std::fs::create_dir_all(directory)
            .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
        self.packages
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                let name = format!("candidate-{index}.rpm");
                std::fs::write(
                    directory.join(&name),
                    cas::load(cache, &candidate.checksum)?,
                )
                .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
                Ok(format!("/work/candidates/{name}"))
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn rpm(
        directory: &Path,
        name: &str,
        version: &str,
        arch: &str,
        dependency: Option<&str>,
    ) -> PathBuf {
        let path = directory.join(format!("{name}-{version}.{arch}.rpm"));
        let mut builder = rpm::PackageBuilder::new(name, version, "MIT", arch, "candidate fixture");
        builder.release("1");
        if let Some(dependency) = dependency {
            builder.requires(rpm::Dependency::any(dependency));
        }
        builder
            .with_file_contents(
                format!("{name}={version}\n").as_bytes(),
                rpm::FileOptions::new(format!("/usr/share/{name}/value")),
            )
            .unwrap();
        builder.build().unwrap().write_file(&path).unwrap();
        path
    }

    #[test]
    fn imported_candidates_survive_original_removal_and_stage_verified_bytes() {
        let work = tempfile::tempdir().unwrap();
        let cache = work.path().join("cas");
        let original = rpm(work.path(), "fixture", "1", "noarch", None);
        let candidates = CandidateSet::import(&cache, std::slice::from_ref(&original)).unwrap();
        std::fs::remove_file(original).unwrap();
        let output = work.path().join("staged");
        assert_eq!(
            candidates.stage(&cache, &output).unwrap(),
            ["/work/candidates/candidate-0.rpm"]
        );
        let metadata = package_metadata(&output.join("candidate-0.rpm")).unwrap();
        assert_eq!(metadata.name, "fixture");
        assert_eq!(metadata.evr(), "1-1");
        let digest = candidates.packages()[0].checksum();
        std::fs::write(cas::path(&cache, digest).unwrap(), b"corrupt").unwrap();
        assert!(candidates
            .stage(&cache, &output)
            .unwrap_err()
            .message
            .contains("corrupt"));
    }

    #[test]
    fn conflicting_versions_and_payloads_are_rejected_but_identical_inputs_deduplicate() {
        let work = tempfile::tempdir().unwrap();
        let cache = work.path().join("cas");
        let first = rpm(work.path(), "fixture", "1", "noarch", None);
        let second = rpm(work.path(), "fixture", "2", "noarch", None);
        assert_eq!(
            CandidateSet::import(&cache, &[first.clone(), first.clone()])
                .unwrap()
                .packages()
                .len(),
            1
        );
        assert!(CandidateSet::import(&cache, &[first, second])
            .unwrap_err()
            .message
            .contains("multiple candidate payloads"));
    }

    #[test]
    fn selecting_outputs_is_explicit_and_does_not_smuggle_siblings() {
        let work = tempfile::tempdir().unwrap();
        let candidates = CandidateSet::import(
            &work.path().join("cas"),
            &[
                rpm(work.path(), "fixture", "1", "noarch", None),
                rpm(work.path(), "fixture-devel", "1", "noarch", None),
            ],
        )
        .unwrap();
        let selected = candidates.select(&["fixture".to_string()]).unwrap();
        assert_eq!(selected.packages().len(), 1);
        assert_eq!(selected.packages()[0].metadata().name, "fixture");
        assert!(candidates
            .select(&["missing".to_string()])
            .unwrap_err()
            .message
            .contains("absent"));
    }

    #[test]
    fn source_rpms_are_not_candidate_packages() {
        let work = tempfile::tempdir().unwrap();
        let path = work.path().join("source.rpm");
        let mut builder =
            rpm::PackageBuilder::new("fixture", "1", "MIT", "noarch", "source fixture");
        builder.source_package(true);
        builder.build().unwrap().write_file(&path).unwrap();
        assert!(CandidateSet::import(&work.path().join("cas"), &[path])
            .unwrap_err()
            .message
            .contains("binary RPMs"));
    }
}

/// Solver-visible candidates with immutable private repository metadata and payloads.
/// Only the outputs explicitly requested by the caller are mandatory.
pub struct CandidateRepository {
    pub(crate) pool: CandidateSet,
    snapshot: tempfile::TempDir,
}

impl CandidateRepository {
    pub fn verify(pool: &CandidateSet, indexed: &Path, cache: &Path) -> Result<Self> {
        if pool.is_empty() {
            return Err(Error::new(
                codes::ROOT_009,
                "candidate repository has no RPMs",
            ));
        }
        let snapshot = tempfile::Builder::new()
            .prefix("rpmx-candidates-")
            .tempdir()
            .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
        let mut found = BTreeSet::new();
        for entry in std::fs::read_dir(indexed)
            .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?
        {
            let entry = entry.map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("rpm") {
                continue;
            }
            let bytes = std::fs::read(entry.path())
                .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
            let checksum = rpmx_base::hash::sha512_hex(&bytes);
            let candidate = pool
                .packages
                .iter()
                .find(|candidate| candidate.checksum == checksum)
                .ok_or_else(|| {
                    Error::new(
                        codes::ROOT_009,
                        format!(
                            "repository RPM {} is not an imported candidate",
                            entry.path().display()
                        ),
                    )
                })?;
            if !found.insert(candidate.checksum.clone()) {
                return Err(Error::new(
                    codes::ROOT_009,
                    "candidate repository contains duplicate RPM payloads",
                ));
            }
            std::fs::write(
                snapshot.path().join(entry.file_name()),
                cas::load(cache, &candidate.checksum)?,
            )
            .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
        }
        if found.len() != pool.packages.len() {
            return Err(Error::new(
                codes::ROOT_009,
                "candidate repository does not contain the complete imported pool",
            ));
        }
        let metadata = indexed.join("repodata");
        if !metadata.join("repomd.xml").is_file() {
            return Err(Error::new(
                codes::ROOT_009,
                "candidate repository has no repodata/repomd.xml",
            ));
        }
        std::fs::create_dir(snapshot.path().join("repodata"))
            .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
        for entry in std::fs::read_dir(metadata)
            .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?
        {
            let entry = entry.map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
            if !entry
                .file_type()
                .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?
                .is_file()
            {
                return Err(Error::new(
                    codes::ROOT_009,
                    "candidate repository metadata must be regular files",
                ));
            }
            std::fs::copy(
                entry.path(),
                snapshot.path().join("repodata").join(entry.file_name()),
            )
            .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
        }
        Ok(Self {
            pool: pool.clone(),
            snapshot,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.snapshot.path()
    }
}

#[cfg(test)]
mod repository_tests {
    use super::*;

    fn metadata(directory: &Path) {
        std::fs::create_dir(directory.join("repodata")).unwrap();
        std::fs::write(directory.join("repodata/repomd.xml"), b"<repomd/>").unwrap();
    }

    #[test]
    fn repository_snapshot_survives_input_removal_and_rejects_unimported_rpms() {
        let work = tempfile::tempdir().unwrap();
        let indexed = work.path().join("indexed");
        std::fs::create_dir(&indexed).unwrap();
        metadata(&indexed);
        let input = tests::rpm(&indexed, "fixture", "1", "noarch", None);
        let cache = work.path().join("cas");
        let pool = CandidateSet::import(&cache, std::slice::from_ref(&input)).unwrap();
        let repository = CandidateRepository::verify(&pool, &indexed, &cache).unwrap();
        let extra = tests::rpm(&indexed, "unimported", "1", "noarch", None);
        assert!(CandidateRepository::verify(&pool, &indexed, &cache)
            .err()
            .unwrap()
            .message
            .contains("not an imported candidate"));
        std::fs::remove_file(extra).unwrap();
        std::fs::remove_dir_all(indexed).unwrap();
        let snapshot = repository.path().join(input.file_name().unwrap());
        assert_eq!(package_metadata(&snapshot).unwrap().name, "fixture");
        assert_eq!(
            std::fs::read(repository.path().join("repodata/repomd.xml")).unwrap(),
            b"<repomd/>"
        );
    }

    #[test]
    fn repository_cannot_omit_or_duplicate_imported_payloads() {
        let work = tempfile::tempdir().unwrap();
        let indexed = work.path().join("indexed");
        std::fs::create_dir(&indexed).unwrap();
        metadata(&indexed);
        let input = tests::rpm(&indexed, "fixture", "1", "noarch", None);
        let cache = work.path().join("cas");
        let pool = CandidateSet::import(&cache, std::slice::from_ref(&input)).unwrap();
        std::fs::copy(&input, indexed.join("duplicate.rpm")).unwrap();
        assert!(CandidateRepository::verify(&pool, &indexed, &cache)
            .err()
            .unwrap()
            .message
            .contains("duplicate"));
        std::fs::remove_file(indexed.join("duplicate.rpm")).unwrap();
        std::fs::remove_file(input).unwrap();
        assert!(CandidateRepository::verify(&pool, &indexed, &cache)
            .err()
            .unwrap()
            .message
            .contains("complete imported pool"));
    }
}
