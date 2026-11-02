//! The runner-owned QEMU transport appliance.
//!
//! A QEMU build must not boot the build root. The appliance is a small RPM
//! closure containing the kernel, init system, network setup and SSH transport.
//! It is resolved from the build profile's repositories, releasever and builder
//! image, but with a package list owned by rpmx rather than by the profile.
//!
//! The appliance never contributes to the build `closure_digest` or to the
//! build root's filesystem view.

use std::path::{Path, PathBuf};

use crate::cache_lock::CacheLock;
use crate::profile::{Packages, Profile, ResolveOptions, DOCUMENT, VERSION};
use crate::root::{
    assemble, remove_tree, resolve_profile, ApplianceLock, AssembleOptions, APPLIANCE_SCHEMA,
};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};
use rpmx_runner::runner::RunnerKind;

/// Whether a root resolution should also resolve the QEMU appliance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Auto,
    Always,
    Never,
}

impl std::str::FromStr for Policy {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            other => Err(Error::new(
                codes::ROOT_006,
                format!("unknown appliance policy '{other}'; expected auto, always or never"),
            )),
        }
    }
}

impl Policy {
    pub fn should_resolve(self, host: Arch, target: Arch, runner: RunnerKind) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => runner == RunnerKind::Qemu || host != target,
        }
    }
}

/// The initial appliance package set.
///
/// Bump [`APPLIANCE_SCHEMA`] when this list changes. The list is deliberately
/// explicit rather than derived: a user package added for a build must never
/// become transport by accident.
pub const APPLIANCE_PACKAGES: &[&str] = &[
    "bash",
    "coreutils",
    "dracut",
    "kmod",
    "kernel-core",
    "kernel-modules-core",
    "kernel-modules",
    "openssh-server",
    "systemd",
    "systemd-networkd",
    "tar",
    "util-linux",
];

/// Build the synthetic profile used to resolve the appliance.
///
/// It inherits the distribution inputs (repositories, releasever and builder
/// image) and replaces everything that describes the build environment.
pub fn profile_for(profile: &Profile, name: &str) -> Profile {
    Profile {
        document: DOCUMENT.to_string(),
        version: VERSION,
        name: Some(format!("{name}-qemu-appliance")),
        releasever: profile.releasever.clone(),
        dist_tag: None,
        archs: profile.archs.clone(),
        builder: profile.builder.clone(),
        repositories: profile.repositories.clone(),
        packages: Packages {
            install: APPLIANCE_PACKAGES
                .iter()
                .map(|package| (*package).to_string())
                .collect(),
            reinstall: Vec::new(),
        },
        modules: Default::default(),
        options: ResolveOptions {
            weak_deps: false,
            docs: false,
        },
        paths: profile.paths.clone(),
        debuginfo: false,
        sbom: false,
    }
}

/// Resolve the appliance closure for a build profile.
pub fn resolve(profile: &Profile, name: &str, work: &Path, cache: &Path) -> Result<ApplianceLock> {
    let appliance_profile = profile_for(profile, name);
    let lock = resolve_profile(
        &appliance_profile,
        &appliance_profile.name(name),
        work,
        cache,
    )?;
    let builder = lock.builder.ok_or_else(|| {
        Error::new(
            codes::ROOT_021,
            "the resolved QEMU appliance has no builder image",
        )
    })?;
    let appliance = ApplianceLock {
        schema: APPLIANCE_SCHEMA,
        profile: lock.profile,
        archs: lock.archs,
        releasever: lock.releasever,
        builder,
        repositories: lock.repositories,
        packages: lock.packages,
        closure_digest: lock.closure_digest,
    };
    appliance.validate()?;
    Ok(appliance)
}

/// Assemble the appliance offline from the CAS.
pub fn assemble_root(lock: &ApplianceLock, options: AssembleOptions<'_>) -> Result<PathBuf> {
    lock.validate()?;
    assemble(&lock.as_root_lock(), options)
}

/// Ensure the locked appliance is assembled in its content-addressed cache.
///
/// A completed cache entry is reused without verifying the tree: the appliance
/// identity is the closure plus the builder digest, and its initramfs is not a
/// build input. The `.complete` marker is written only after assembly and
/// package verification succeed.
pub fn ensure_cached(lock: &ApplianceLock, cache: &Path, offline: bool) -> Result<PathBuf> {
    lock.validate()?;
    let identity = lock.identity();
    let root = cache_root(cache, &identity);
    let directory = root.parent().unwrap_or(&root).to_path_buf();
    let marker = directory.join(".complete");
    if marker.is_file() && root.is_dir() {
        return Ok(root);
    }
    if offline {
        return Err(Error::new(
            codes::ROOT_020,
            format!(
                "the QEMU appliance is not cached at {}; run without --offline to assemble it",
                directory.display()
            ),
        ));
    }
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot create {}: {error}", directory.display()),
        )
    })?;
    let key = identity.strip_prefix("sha512:").unwrap_or(&identity);
    let _publishing = CacheLock::acquire(
        directory.parent().unwrap_or(&directory),
        &format!("appliance-{key}"),
    )?;
    if marker.is_file() && root.is_dir() {
        return Ok(root);
    }
    let _ = std::fs::remove_file(&marker);
    if root.exists() {
        remove_tree(&root)?;
    }
    let work = tempfile::Builder::new()
        .prefix("rpmx-appliance-")
        .tempdir_in(directory.parent().unwrap_or(&directory))
        .map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot create an appliance work directory: {error}"),
            )
        })?;
    assemble_root(
        lock,
        AssembleOptions {
            cache,
            work: work.path(),
            root: &root,
        },
    )?;
    std::fs::write(&marker, &identity).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot write {}: {error}", marker.display()),
        )
    })?;
    Ok(root)
}

/// Cache directory for one appliance identity.
pub fn cache_root(cache: &Path, identity: &str) -> PathBuf {
    let key = identity.strip_prefix("sha512:").unwrap_or(identity);
    cache
        .parent()
        .unwrap_or(cache)
        .join("appliances")
        .join(key)
        .join("root")
}

#[cfg(test)]
mod tests {
    use super::{profile_for, APPLIANCE_PACKAGES};
    use crate::profile::{
        Builder, Packages, Profile, Repository, ResolveOptions, DOCUMENT, VERSION,
    };
    use crate::root::{
        closure_digest, load_lock, write_lock, ApplianceLock, LockedBuilder, LockedPackage,
        APPLIANCE_SCHEMA,
    };
    use std::collections::BTreeMap;

    fn repository() -> Repository {
        Repository {
            id: "fedora".to_string(),
            baseurl: None,
            metalink: Some(
                "https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever&arch=$basearch"
                    .to_string(),
            ),
            mirrorlist: None,
            gpgkey: None,
            repo_gpgcheck: false,
            auth: None,
        }
    }

    fn profile() -> Profile {
        Profile {
            document: DOCUMENT.to_string(),
            version: VERSION,
            name: Some("fedora-44".to_string()),
            releasever: "44".to_string(),
            dist_tag: Some(".fc44".to_string()),
            archs: vec!["x86_64".to_string()],
            builder: Builder {
                image: Some("registry.fedoraproject.org/fedora:44".to_string()),
            },
            repositories: vec![repository()],
            packages: Packages {
                install: vec!["gcc".to_string()],
                reinstall: Vec::new(),
            },
            modules: Default::default(),
            options: ResolveOptions {
                weak_deps: true,
                docs: true,
            },
            paths: Default::default(),
            debuginfo: true,
            sbom: true,
        }
    }

    fn package(name: &str, checksum: &str) -> LockedPackage {
        LockedPackage {
            name: name.to_string(),
            repo_id: "fedora".to_string(),
            evr: "1-1.fc44".to_string(),
            checksum: checksum.to_string(),
            size: 1,
            srpm: None,
            module: None,
            location: None,
        }
    }

    fn appliance() -> ApplianceLock {
        let archs = vec!["x86_64".to_string()];
        let mut packages = BTreeMap::new();
        packages.insert(
            "x86_64".to_string(),
            vec![package("kernel-core", "sha512:aa")],
        );
        let repositories = vec![repository()];
        let closure = closure_digest("44", &archs, &repositories, &packages);
        ApplianceLock {
            schema: APPLIANCE_SCHEMA,
            profile: "fedora-44-qemu-appliance".to_string(),
            archs,
            releasever: "44".to_string(),
            builder: LockedBuilder {
                image: "registry.fedoraproject.org/fedora:44".to_string(),
                image_digest: "sha256:builder".to_string(),
            },
            repositories,
            packages,
            closure_digest: closure,
        }
    }

    #[test]
    fn the_policy_decides_when_a_vm_needs_an_appliance() {
        use crate::appliance::Policy;
        use rpmx_base::arch::Arch;
        use rpmx_runner::runner::RunnerKind;

        assert!(Policy::Always.should_resolve(Arch::X86_64, Arch::X86_64, RunnerKind::Bwrap));
        assert!(!Policy::Never.should_resolve(Arch::X86_64, Arch::Aarch64, RunnerKind::Qemu));
        assert!(Policy::Auto.should_resolve(Arch::X86_64, Arch::Aarch64, RunnerKind::Bwrap));
        assert!(Policy::Auto.should_resolve(Arch::X86_64, Arch::X86_64, RunnerKind::Qemu));
        assert!(!Policy::Auto.should_resolve(Arch::X86_64, Arch::X86_64, RunnerKind::Bwrap));
    }

    #[test]
    fn appliance_profile_inherits_distribution_inputs() {
        let profile = profile_for(&profile(), "fedora-44");
        assert_eq!(profile.releasever, "44");
        assert_eq!(profile.archs, ["x86_64"]);
        assert_eq!(
            profile.builder.image.as_deref(),
            Some("registry.fedoraproject.org/fedora:44")
        );
        assert_eq!(profile.repositories.len(), 1);
        assert_eq!(profile.repositories[0].id, "fedora");
        assert_eq!(
            profile.repositories[0].metalink.as_deref(),
            Some(
                "https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever&arch=$basearch"
            )
        );
        assert_eq!(
            profile.packages.install,
            APPLIANCE_PACKAGES
                .iter()
                .map(|package| (*package).to_string())
                .collect::<Vec<_>>()
        );
        assert!(!profile.options.weak_deps);
        assert!(!profile.options.docs);
        assert!(!profile.debuginfo);
        assert!(!profile.sbom);
        assert_eq!(profile.dist_tag, None);
    }

    #[test]
    fn appliance_package_list_is_a_snapshot() {
        // Changing this list requires a schema bump. That is intentional: an
        // old lock must not silently boot an appliance with a different
        // contract.
        assert_eq!(
            crate::root::APPLIANCE_SCHEMA,
            1,
            "changing the package list requires bumping APPLIANCE_SCHEMA"
        );
        assert_eq!(
            APPLIANCE_PACKAGES,
            &[
                "bash",
                "coreutils",
                "dracut",
                "kmod",
                "kernel-core",
                "kernel-modules-core",
                "kernel-modules",
                "openssh-server",
                "systemd",
                "systemd-networkd",
                "tar",
                "util-linux",
            ]
        );
    }

    #[test]
    fn a_tampered_appliance_closure_is_rejected() {
        let mut lock = appliance();
        lock.packages
            .get_mut("x86_64")
            .expect("packages")
            .push(package("openssh-server", "sha512:bb"));
        let error = lock.validate().expect_err("tampered closure");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(error.message.contains("closure digest"));
    }

    #[test]
    fn an_unsupported_appliance_schema_is_rejected() {
        let mut lock = appliance();
        lock.schema += 1;
        let error = lock.validate().expect_err("schema");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(error.message.contains("schema"));
    }

    #[test]
    fn identity_tracks_schema_closure_and_builder() {
        let lock = appliance();
        let identity = lock.identity();
        assert!(identity.starts_with("sha512:"));

        let mut changed = lock.clone();
        changed.schema += 1;
        assert_ne!(changed.identity(), identity);

        let mut changed = lock.clone();
        changed.builder.image_digest = "sha256:other".to_string();
        assert_ne!(changed.identity(), identity);

        let mut changed = lock.clone();
        changed.closure_digest = "sha512:other".to_string();
        assert_ne!(changed.identity(), identity);
    }

    #[test]
    fn a_root_lock_without_an_appliance_still_round_trips() {
        let lock = appliance().as_root_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("root.lock.yaml");
        write_lock(&path, &lock).expect("write");
        let loaded = load_lock(&path).expect("load");
        assert!(loaded.qemu_appliance().is_none());
    }

    #[test]
    fn a_root_lock_with_an_appliance_round_trips() {
        let appliance = appliance();
        let mut lock = appliance.as_root_lock();
        lock.set_qemu_appliance(Some(appliance));
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("root.lock.yaml");
        write_lock(&path, &lock).expect("write");
        let loaded = load_lock(&path).expect("load");
        assert_eq!(
            loaded.qemu_appliance().expect("appliance").closure_digest,
            lock.qemu_appliance().expect("appliance").closure_digest
        );
    }

    #[test]
    fn a_tampered_appliance_in_a_root_lock_is_rejected_on_load() {
        let appliance = appliance();
        let mut lock = appliance.as_root_lock();
        lock.set_qemu_appliance(Some(appliance));
        lock.runner_assets
            .as_mut()
            .and_then(|assets| assets.qemu_appliance.as_mut())
            .expect("appliance")
            .packages
            .get_mut("x86_64")
            .expect("packages")
            .clear();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("root.lock.yaml");
        // Write directly: this test needs a document that write_lock would
        // refuse, so it can exercise the load path.
        std::fs::write(&path, serde_yaml_ng::to_string(&lock).expect("serialize")).expect("write");
        let error = load_lock(&path).expect_err("tampered lock");
        assert_eq!(error.code, "RPMX-ROOT-021");
    }

    #[test]
    fn an_empty_appliance_closure_is_rejected_even_with_a_matching_digest() {
        let mut lock = appliance();
        lock.packages.clear();
        lock.packages.insert("x86_64".to_string(), Vec::new());
        lock.closure_digest = closure_digest(
            &lock.releasever,
            &lock.archs,
            &lock.repositories,
            &lock.packages,
        );
        let error = lock.validate().expect_err("empty closure");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(error.message.contains("no packages"));
    }

    #[test]
    fn an_unsafe_appliance_package_name_is_rejected() {
        let mut lock = appliance();
        lock.packages.get_mut("x86_64").expect("packages")[0].name = "../evil".to_string();
        lock.closure_digest = closure_digest(
            &lock.releasever,
            &lock.archs,
            &lock.repositories,
            &lock.packages,
        );
        let error = lock.validate().expect_err("unsafe name");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(error.message.contains("staged safely"));
    }

    #[test]
    fn an_unsafe_appliance_evr_is_rejected() {
        let mut lock = appliance();
        lock.packages.get_mut("x86_64").expect("packages")[0].evr = "../evil".to_string();
        lock.closure_digest = closure_digest(
            &lock.releasever,
            &lock.archs,
            &lock.repositories,
            &lock.packages,
        );
        let error = lock.validate().expect_err("unsafe EVR");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(error.message.contains("staged safely"));
    }

    #[test]
    fn an_invalid_appliance_checksum_is_rejected() {
        let mut lock = appliance();
        lock.packages.get_mut("x86_64").expect("packages")[0].checksum = "sha512:zz".to_string();
        lock.closure_digest = closure_digest(
            &lock.releasever,
            &lock.archs,
            &lock.repositories,
            &lock.packages,
        );
        let error = lock.validate().expect_err("invalid checksum");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(error.message.contains("staged safely"));
    }

    #[test]
    fn an_old_appliance_schema_is_rejected() {
        let mut lock = appliance();
        lock.schema = 0;
        let error = lock.validate().expect_err("old schema");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(error.message.contains("schema"));
    }

    #[test]
    fn an_unknown_appliance_field_is_root_021() {
        let appliance = appliance();
        let mut lock = appliance.as_root_lock();
        lock.set_qemu_appliance(Some(appliance));
        let text = serde_yaml_ng::to_string(&lock).expect("serialize");
        let text = text.replace("runner-assets:\n", "runner-assets:\n  bogus: 1\n");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("root.lock.yaml");
        std::fs::write(&path, text).expect("write");
        let error = load_lock(&path).expect_err("unknown runner asset field");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(error.message.contains("invalid runner assets"));
    }

    #[test]
    fn a_cross_architecture_appliance_is_root_021() {
        let mut cross = appliance();
        let entries = cross.packages.remove("x86_64").expect("x86_64 packages");
        cross.archs = vec!["aarch64".to_string()];
        cross.packages.insert("aarch64".to_string(), entries);
        cross.closure_digest = closure_digest(
            &cross.releasever,
            &cross.archs,
            &cross.repositories,
            &cross.packages,
        );

        let mut lock = appliance().as_root_lock();
        lock.set_qemu_appliance(Some(cross));
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("root.lock.yaml");
        // write_lock also rejects this, so write the raw document.
        std::fs::write(&path, serde_yaml_ng::to_string(&lock).expect("serialize")).expect("write");
        let error = load_lock(&path).expect_err("cross-architecture appliance");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(error.message.contains("architecture"));
    }

    #[test]
    fn write_lock_rejects_an_invalid_appliance_before_writing() {
        let mut appliance = appliance();
        appliance.schema += 1;
        let mut lock = appliance.as_root_lock();
        lock.set_qemu_appliance(Some(appliance));
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("root.lock.yaml");
        let error = write_lock(&path, &lock).expect_err("invalid appliance");
        assert_eq!(error.code, "RPMX-ROOT-021");
        assert!(!path.exists(), "an invalid lock must not be written");
    }
}
