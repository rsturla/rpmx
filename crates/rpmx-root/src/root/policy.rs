//! Target policy for package acceptance. This is policy, never an empty root lock.
use crate::profile::Profile;
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};

#[derive(Clone)]
pub struct RootPolicy {
    pub(crate) profile: Profile,
    pub(crate) images: Option<crate::root::LockedBuilder>,
}

impl RootPolicy {
    /// Select target metadata without inheriting build-profile seed packages.
    pub fn from_profile(profile: &Profile, target: Arch) -> Result<Self> {
        if !profile
            .archs
            .iter()
            .any(|arch| Arch::parse(arch).ok() == Some(target))
        {
            return Err(Error::new(
                codes::ROOT_006,
                "profile does not support the requested target",
            ));
        }
        let mut profile = profile.clone();
        profile.archs = vec![target.to_string()];
        profile.packages.install.clear();
        profile.packages.reinstall.clear();
        profile.cross = None;
        Ok(Self {
            profile,
            images: None,
        })
    }
    /// Reuse locked repository/layout policy and immutable tool images, never its package seeds.
    pub fn from_lock(lock: &crate::root::RootLock) -> Result<Self> {
        lock.validate()?;
        let images = lock.builder.clone().ok_or_else(|| {
            Error::new(
                codes::ROOT_006,
                "root lock has no tool images; supply an explicit profile",
            )
        })?;
        let profile = Profile {
            document: crate::profile::DOCUMENT.to_string(),
            version: crate::profile::VERSION,
            name: Some(lock.profile.clone()),
            releasever: lock.releasever.clone(),
            dist_tag: lock.dist_tag.clone(),
            archs: lock.archs.clone(),
            builder: crate::profile::Builder {
                image: Some(images.installer.reference().to_string()),
            },
            repositories: lock.repositories.clone(),
            packages: Default::default(),
            cross: None,
            modules: Default::default(),
            options: lock.options.clone(),
            paths: crate::profile::Paths {
                prefix: lock.prefix.clone(),
                libdir: lock.libdir.clone(),
                libdir_by_arch: Default::default(),
            },
            sbom: false,
        };
        Ok(Self {
            profile,
            images: Some(images),
        })
    }

    pub fn target(&self) -> Arch {
        Arch::parse(self.profile.arch()).expect("validated target")
    }
    pub fn prefix(&self) -> &str {
        &self.profile.paths.prefix
    }
    pub fn libdir(&self) -> &str {
        self.profile.paths.libdir_for(self.profile.arch())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn target_policy_does_not_inherit_build_seeds() {
        let profile = crate::profile::parse_profile(
            r#"
document: rpmx-profile
version: 1
archs: [x86_64, aarch64]
releasever: '44'
repositories:
  - id: production
    baseurl: https://example.invalid/$basearch
packages:
  install: [gcc, make]
paths:
  libdir-by-arch:
    aarch64: /usr/lib
"#,
        )
        .unwrap();
        let policy =
            super::RootPolicy::from_profile(&profile, rpmx_base::arch::Arch::Aarch64).unwrap();
        assert!(policy.profile.packages.install.is_empty());
        assert_eq!(policy.libdir(), "/usr/lib");
        assert_eq!(profile.packages.install, ["gcc", "make"]);
        assert!(super::RootPolicy::from_profile(&profile, rpmx_base::arch::Arch::S390x).is_err());
    }
}
