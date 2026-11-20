//! Check declared requirements against the shared, verified input inventory.
use rpmx_base::error::{codes, Error, Result};

use super::{RequirementStatus, VerifiedInputs};

pub fn check_build_requirements(
    build: &VerifiedInputs,
    sysroot: Option<&VerifiedInputs>,
    packages: &[String],
    target_packages: &[String],
) -> Result<()> {
    check(build, packages, "build root")?;
    match sysroot {
        Some(root) => check(root, target_packages, "target sysroot"),
        None => check(build, target_packages, "build root"),
    }
}

fn check(inputs: &VerifiedInputs, requirements: &[String], role: &str) -> Result<()> {
    let mut missing = Vec::new();
    let mut unknown = Vec::new();
    for requirement in requirements {
        match inputs.requirement(requirement)? {
            RequirementStatus::Satisfied => {}
            RequirementStatus::Unsatisfied => missing.push(requirement.as_str()),
            RequirementStatus::Unknown => unknown.push(requirement.as_str()),
        }
    }
    let mut failures = Vec::new();
    if !missing.is_empty() {
        failures.push(format!(
            "RPM requirements not satisfied by the pinned {role}: {}",
            missing.join(", ")
        ));
    }
    if !unknown.is_empty() {
        failures.push(format!("cannot verify RPM requirements for the pinned {role}: {}; input RPM metadata is incomplete; retain cached RPMs to verify virtual capabilities", unknown.join(", ")));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::new(codes::ROOT_018, failures.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{closure_digest, LockedPackage, PackageOrigin, RootLock};
    use rpmx_store::cas;
    use std::path::Path;

    fn check_build_requirements(
        build: &RootLock,
        sysroot: Option<&RootLock>,
        packages: &[String],
        target: &[String],
        cache: &Path,
    ) -> Result<()> {
        let build = VerifiedInputs::load(build, cache)?;
        let sysroot = sysroot
            .map(|lock| VerifiedInputs::load(lock, cache))
            .transpose()?;
        super::check_build_requirements(&build, sysroot.as_ref(), packages, target)
    }
    use std::collections::BTreeMap;

    fn lock(cache: &Path, name: &str, capability: &str, version: &str) -> RootLock {
        let mut builder = rpm::PackageBuilder::new(name, "1", "MIT", "noarch", "provider fixture");
        builder
            .release("1")
            .provides(rpm::Dependency::eq(capability, version));
        let path = cache.join(format!("{name}.rpm"));
        builder.build().unwrap().write_file(&path).unwrap();
        let bytes = std::fs::read(path).unwrap();
        let checksum = cas::store(cache, &bytes).unwrap();
        let packages: BTreeMap<String, Vec<LockedPackage>> = BTreeMap::from([(
            "noarch".into(),
            vec![LockedPackage {
                name: name.into(),
                evr: "1-1".into(),
                checksum,
                origin: PackageOrigin::Candidate,
                size: bytes.len() as u64,
                srpm: None,
                module: None,
            }],
        )]);
        let mut lock: RootLock = serde_json::from_value(serde_json::json!({
            "document": "rpmx-root-lock", "version": 2, "profile": "fixture",
            "archs": ["x86_64"], "releasever": "44", "repositories": [],
            "packages": packages, "closure_digest": "",
            "root_digest": format!("sha512:{}", "0".repeat(128))
        }))
        .unwrap();
        lock.closure_digest = closure_digest(
            &lock.releasever,
            &lock.archs,
            &lock.repositories,
            &lock.packages,
        );
        lock
    }

    #[test]
    fn assembled_receipts_do_not_hide_virtual_capabilities_or_version_failures() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path();
        let build = lock(cache, "gcc-16-default", "gcc-c++", "16.2-3");
        check_build_requirements(&build, None, &["gcc-c++ >= 16".into()], &[], cache).unwrap();
        // A native target requirement is satisfied from the same root.
        check_build_requirements(&build, None, &[], &["gcc-c++ = 16.2".into()], cache).unwrap();
        for requirement in ["gcc-c++ >= 17", "absent"] {
            let error = check_build_requirements(&build, None, &[requirement.into()], &[], cache)
                .unwrap_err();
            assert!(error.to_string().contains(requirement));
        }
        // Renaming the package to the requested capability is insufficient.
        let old = lock(cache, "gcc-c++", "other", "1");
        assert!(
            check_build_requirements(&old, None, &["gcc-c++ >= 16".into()], &[], cache).is_err()
        );
    }

    #[test]
    fn assembled_roots_retain_package_identity_checks_without_cached_rpms() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path();
        let build = lock(cache, "compiler-default", "gcc-c++", "16");
        std::fs::remove_file(cas::path(cache, &build.packages["noarch"][0].checksum).unwrap())
            .unwrap();
        check_build_requirements(&build, None, &["compiler-default = 1".into()], &[], cache)
            .unwrap();
        let error =
            check_build_requirements(&build, None, &["gcc-c++".into()], &[], cache).unwrap_err();
        assert!(error.to_string().contains("metadata is incomplete"));
    }

    #[test]
    fn cross_roots_validate_tools_and_target_dependencies_separately() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path();
        let build = lock(cache, "compiler-default", "gcc-c++", "16.2");
        let target = lock(cache, "libstdc++-16-devel", "libstdc++-devel", "16.2");
        check_build_requirements(
            &build,
            Some(&target),
            &["gcc-c++ >= 16".into()],
            &["libstdc++-devel >= 16".into()],
            cache,
        )
        .unwrap();
        let error = check_build_requirements(
            &build,
            Some(&target),
            &["libstdc++-devel".into()],
            &[],
            cache,
        )
        .unwrap_err();
        assert!(error.to_string().contains("build root"));
        let error =
            check_build_requirements(&build, Some(&target), &[], &["gcc-c++".into()], cache)
                .unwrap_err();
        assert!(error.to_string().contains("target sysroot"));
        let package = &build.packages["noarch"][0];
        std::fs::write(
            cas::path(cache, &package.checksum).unwrap(),
            b"changed bytes",
        )
        .unwrap();
        assert!(check_build_requirements(&build, None, &["gcc-c++".into()], &[], cache).is_err());
    }
}
