//! One interpretation of pinned RPM inputs for assembly, checks and provenance.
use std::path::Path;

use rpmx_base::error::{codes, Error, Result};
use rpmx_payload::{
    capabilities::Capabilities,
    metadata::{package_metadata_from_bytes, PackageMetadata},
};
use rpmx_store::cas;

use super::{LockedPackage, RootLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementStatus {
    Satisfied,
    Unsatisfied,
    /// No known provider, but one or more locked RPM headers are unavailable.
    Unknown,
}

/// A locked identity with optional metadata from checksum-verified RPM bytes.
/// Missing headers retain the lock's identity, without claiming its capabilities.
pub struct VerifiedInput {
    locked: LockedPackage,
    arch: String,
    metadata: Option<PackageMetadata>,
}

impl VerifiedInput {
    pub fn locked(&self) -> &LockedPackage {
        &self.locked
    }

    pub fn arch(&self) -> &str {
        &self.arch
    }

    pub fn metadata(&self) -> Option<&PackageMetadata> {
        self.metadata.as_ref()
    }
}

/// Immutable metadata snapshot of a validated lock's exact package selection.
/// Coverage depends only on cached RPMs, never on filesystem assembly receipts.
pub struct VerifiedInputs {
    packages: Vec<VerifiedInput>,
    capabilities: Capabilities,
}

impl VerifiedInputs {
    pub fn load(lock: &RootLock, cache: &Path) -> Result<Self> {
        lock.validate()?;
        let mut packages = Vec::new();
        let mut capabilities = Capabilities::default();
        for (arch, entries) in &lock.packages {
            for locked in entries {
                let path = cas::path(cache, &locked.checksum)?;
                let metadata = if path.try_exists().map_err(|error| {
                    Error::new(
                        codes::ROOT_012,
                        format!("cannot inspect {}: {error}", path.display()),
                    )
                })? {
                    let bytes = cas::load(cache, &locked.checksum)?;
                    let metadata = package_metadata_from_bytes(&bytes)?;
                    if metadata.name != locked.name
                        || metadata.evr() != locked.evr
                        || metadata.arch != *arch
                    {
                        return Err(Error::new(
                            codes::ROOT_012,
                            format!(
                                "locked identity does not match RPM payload {}",
                                locked.checksum
                            ),
                        ));
                    }
                    capabilities.insert(&metadata);
                    Some(metadata)
                } else {
                    capabilities.extend([rpm::Dependency::eq(&locked.name, &locked.evr)]);
                    None
                };
                packages.push(VerifiedInput {
                    locked: locked.clone(),
                    arch: arch.clone(),
                    metadata,
                });
            }
        }
        Ok(Self {
            packages,
            capabilities,
        })
    }

    pub fn packages(&self) -> &[VerifiedInput] {
        &self.packages
    }

    pub fn complete(&self) -> bool {
        self.packages
            .iter()
            .all(|package| package.metadata.is_some())
    }

    /// Assembly requires every RPM; receipt reuse may tolerate missing headers.
    pub fn require_complete(&self) -> Result<()> {
        if let Some(package) = self
            .packages
            .iter()
            .find(|package| package.metadata.is_none())
        {
            return Err(Error::new(
                codes::ROOT_011,
                format!("payload {} is not in the cache", package.locked.checksum),
            ));
        }
        Ok(())
    }

    pub fn requirement(&self, requirement: &str) -> Result<RequirementStatus> {
        Ok(if self.capabilities.satisfies(requirement)? {
            RequirementStatus::Satisfied
        } else if self.complete() {
            RequirementStatus::Unsatisfied
        } else {
            RequirementStatus::Unknown
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{closure_digest, CandidateSet};

    fn fixture(cache: &Path) -> RootLock {
        let rpm = super::super::candidate::tests::rpm(cache, "fixture", "1", "noarch", None);
        let candidates = CandidateSet::import(cache, &[rpm]).unwrap();
        super::super::lock::model_tests::candidate_lock(&candidates.packages()[0])
    }

    fn reseal(lock: &mut RootLock) {
        lock.closure_digest = closure_digest(
            &lock.releasever,
            &lock.archs,
            &lock.repositories,
            &lock.packages,
        );
    }

    #[test]
    fn coverage_and_requirement_results_do_not_depend_on_assembly_receipts() {
        let work = tempfile::tempdir().unwrap();
        let cache = work.path();
        let mut lock = fixture(cache);
        let payload = cas::path(cache, &lock.packages["noarch"][0].checksum).unwrap();
        let bytes = std::fs::read(&payload).unwrap();
        for cached in [true, false] {
            if cached {
                std::fs::write(&payload, &bytes).unwrap();
            } else {
                std::fs::remove_file(&payload).unwrap();
            }
            for assembled in [false, true] {
                lock.root_digest = assembled.then(|| format!("sha512:{}", "0".repeat(128)));
                let inventory = VerifiedInputs::load(&lock, cache).unwrap();
                assert_eq!(inventory.complete(), cached);
                assert_eq!(inventory.require_complete().is_ok(), cached);
                assert_eq!(
                    inventory.requirement("fixture = 1").unwrap(),
                    RequirementStatus::Satisfied
                );
                assert_eq!(
                    inventory.requirement("absent").unwrap(),
                    if cached {
                        RequirementStatus::Unsatisfied
                    } else {
                        RequirementStatus::Unknown
                    }
                );
                assert_eq!(inventory.packages()[0].metadata().is_some(), cached);
            }
        }
    }

    #[test]
    fn corrupt_bytes_and_mismatched_name_evr_or_arch_are_rejected() {
        let work = tempfile::tempdir().unwrap();
        let cache = work.path();
        let lock = fixture(cache);
        let payload = cas::path(cache, &lock.packages["noarch"][0].checksum).unwrap();
        let bytes = std::fs::read(&payload).unwrap();
        std::fs::write(&payload, b"corrupt").unwrap();
        assert!(VerifiedInputs::load(&lock, cache)
            .err()
            .unwrap()
            .message
            .contains("corrupt"));
        std::fs::write(&payload, bytes).unwrap();
        for field in ["name", "evr", "arch"] {
            let mut changed = lock.clone();
            match field {
                "name" => {
                    changed.packages.get_mut("noarch").unwrap()[0].name = "substituted".into()
                }
                "evr" => changed.packages.get_mut("noarch").unwrap()[0].evr = "2-1".into(),
                _ => {
                    let packages = changed.packages.remove("noarch").unwrap();
                    changed.packages.insert(changed.archs[0].clone(), packages);
                }
            }
            reseal(&mut changed);
            assert!(
                VerifiedInputs::load(&changed, cache)
                    .err()
                    .unwrap()
                    .message
                    .contains("identity does not match"),
                "{field}"
            );
        }
    }

    #[test]
    fn invalid_lock_selection_is_rejected_even_without_cached_headers() {
        let work = tempfile::tempdir().unwrap();
        let mut lock = fixture(work.path());
        lock.packages.get_mut("noarch").unwrap()[0].evr = "2-1".into();
        assert!(VerifiedInputs::load(&lock, work.path())
            .err()
            .unwrap()
            .message
            .contains("closure digest mismatch"));
    }
}
