use serde::{Deserialize, Serialize};

use crate::error::{codes, Error, Result};
use crate::reject_yaml_constructs;

pub const DOCUMENT: &str = "rpmx-profile";
pub const VERSION: u32 = 1;

/// A build-root profile: what to install, and where from.
///
/// The repository and package vocabulary follows the libpkgmanifest
/// `rpm-package-input` document so a profile can be translated mechanically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub document: String,
    pub version: u32,
    /// Stable profile name used in locks and digests. Defaults to the file stem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub releasever: String,
    /// Distribution tag appended to package releases, such as `.fc44`.
    #[serde(default, rename = "dist-tag", skip_serializing_if = "Option::is_none")]
    pub dist_tag: Option<String>,
    pub archs: Vec<String>,
    #[serde(default)]
    pub builder: Builder,
    pub repositories: Vec<Repository>,
    pub packages: Packages,
    #[serde(default)]
    pub modules: Modules,
    #[serde(default)]
    pub options: ResolveOptions,
    /// Package debug sections into a `-debuginfo` output.
    #[serde(default)]
    pub debuginfo: bool,
    /// Emit SPDX SBOMs (per package and build-level) with each build.
    #[serde(default = "default_sbom")]
    pub sbom: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Builder {
    /// Builder image used to run dnf. Defaults to the Fedora image for `releasever`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseurl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metalink: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirrorlist: Option<String>,
    /// Repository key used to verify package signatures. Without it, the
    /// repository is recorded as unsigned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpgkey: Option<String>,
}

impl Repository {
    pub fn location(&self) -> (&'static str, &str) {
        if let Some(url) = &self.baseurl {
            ("baseurl", url)
        } else if let Some(url) = &self.metalink {
            ("metalink", url)
        } else if let Some(url) = &self.mirrorlist {
            ("mirrorlist", url)
        } else {
            ("", "")
        }
    }

    pub fn gpgcheck(&self) -> bool {
        self.gpgkey.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packages {
    pub install: Vec<String>,
    #[serde(default)]
    pub reinstall: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modules {
    #[serde(default)]
    pub enable: Vec<String>,
    #[serde(default)]
    pub disable: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveOptions {
    #[serde(default)]
    pub weak_deps: bool,
    #[serde(default)]
    pub docs: bool,
}

pub fn parse_profile(text: &str) -> Result<Profile> {
    reject_yaml_constructs(text)?;
    let profile: Profile = serde_yaml_ng::from_str(text).map_err(|error| {
        let location = error.location();
        let mapped = Error::new(codes::ROOT_006, error.to_string());
        match location {
            Some(location) => mapped.with_location(Some(location.line()), Some(location.column())),
            None => mapped,
        }
    })?;
    profile.validate()?;
    Ok(profile)
}

pub fn load_profile(path: &std::path::Path) -> Result<Profile> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::ROOT_006,
            format!("cannot read profile {}: {error}", path.display()),
        )
    })?;
    parse_profile(&text)
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        if self.document != DOCUMENT {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "profile document must be '{DOCUMENT}', found '{}'",
                    self.document
                ),
            ));
        }
        if self.version != VERSION {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "unsupported profile version {}; this rpmx understands {VERSION}",
                    self.version
                ),
            ));
        }
        if self.releasever.trim().is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "profile releasever must not be empty",
            ));
        }
        if let Some(tag) = &self.dist_tag {
            crate::plan::normalize_dist_tag(tag)
                .map_err(|error| Error::new(codes::ROOT_006, error.message))?;
        }
        if self.archs.is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "profile must declare at least one architecture",
            ));
        }
        if self.archs.len() > 1 {
            return Err(Error::new(
                codes::ROOT_006,
                "multi-architecture roots are not supported yet; declare one arch",
            ));
        }
        for arch in &self.archs {
            normalize_arch(arch)?;
        }
        if self.repositories.is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "profile must declare at least one repository",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for repository in &self.repositories {
            if repository.id.trim().is_empty() {
                return Err(Error::new(
                    codes::ROOT_006,
                    "repository id must not be empty",
                ));
            }
            if !seen.insert(repository.id.as_str()) {
                return Err(Error::new(
                    codes::ROOT_006,
                    format!("duplicate repository id '{}'", repository.id),
                ));
            }
            let sources = [
                repository.baseurl.is_some(),
                repository.metalink.is_some(),
                repository.mirrorlist.is_some(),
            ];
            if sources.iter().filter(|present| **present).count() != 1 {
                return Err(Error::new(
                    codes::ROOT_006,
                    format!(
                        "repository '{}' must set exactly one of baseurl, metalink or mirrorlist",
                        repository.id
                    ),
                ));
            }
        }
        if self.packages.install.is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "profile must install at least one package",
            ));
        }
        if !self.packages.reinstall.is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "package reinstall is not supported yet",
            ));
        }
        if !self.modules.enable.is_empty() || !self.modules.disable.is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "module streams are not supported yet",
            ));
        }
        Ok(())
    }

    pub fn arch(&self) -> &'static str {
        // Validation guarantees exactly one entry and accepts it.
        normalize_arch(&self.archs[0]).expect("validated profile arch")
    }

    pub fn builder_image(&self) -> String {
        self.builder
            .image
            .clone()
            .unwrap_or_else(|| format!("registry.fedoraproject.org/fedora:{}", self.releasever))
    }

    pub fn name(&self, fallback: &str) -> String {
        self.name.clone().unwrap_or_else(|| fallback.to_string())
    }

    /// The normalized dist tag, if the profile sets one.
    pub fn dist_tag(&self) -> Result<Option<String>> {
        self.dist_tag
            .as_deref()
            .map(crate::plan::normalize_dist_tag)
            .transpose()
    }
}

/// Normalize an architecture name to the RPM spelling used in NEVRAs.
pub fn default_sbom() -> bool {
    true
}

pub fn normalize_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" | "amd64" => Ok("x86_64"),
        "aarch64" | "arm64" => Ok("aarch64"),
        "ppc64le" => Ok("ppc64le"),
        "s390x" => Ok("s390x"),
        "i686" | "i386" => Ok("i686"),
        other => Err(Error::new(
            codes::ROOT_009,
            format!("unsupported architecture '{other}'"),
        )),
    }
}
