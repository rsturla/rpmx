use serde::{Deserialize, Serialize};

use rpmx_base::error::{codes, Error, Result};
use rpmx_base::yaml;
pub use rpmx_language::model::RepositoryAuth;

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
    /// Cross-compilation toolchain. When present, `packages.install` is the
    /// target sysroot and `cross.packages` is the host build root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross: Option<Cross>,
    #[serde(default)]
    pub modules: Modules,
    #[serde(default)]
    pub options: ResolveOptions,
    /// Install layout used by every step in this build.
    #[serde(default)]
    pub paths: Paths,
    /// Package debug sections into a `-debuginfo` output.
    #[serde(default)]
    pub debuginfo: bool,
    /// Emit SPDX SBOMs (per package and build-level) with each build.
    #[serde(default = "default_sbom")]
    pub sbom: bool,
}

/// Where the build installs files. The executor exports these values to every
/// step as `PREFIX` and `LIBDIR`; no step definition may spell them out.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paths {
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default = "default_libdir")]
    pub libdir: String,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            prefix: default_prefix(),
            libdir: default_libdir(),
        }
    }
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
    /// Verify signed repository metadata in addition to package signatures.
    #[serde(default, rename = "repo-gpgcheck")]
    pub repo_gpgcheck: bool,
    /// Credential references for an authenticated repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<RepositoryAuth>,
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

/// A cross-compilation toolchain.
///
/// With a `cross` block the profile describes two roots: a **build root** that
/// runs on the host and holds the compiler, and a **sysroot** for the target
/// that holds the headers and libraries the output links against. Steps run
/// natively on the host; nothing is emulated.
///
/// `packages.install` remains the sysroot. `cross.packages` is the build root,
/// because a target `-devel` package cannot be installed into a host root.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cross {
    /// Architecture of the build root. It must be the host running rpmx.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// GNU triple naming the toolchain commands, such as `aarch64-linux-gnu`.
    /// Defaults to the target architecture's triple.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triple: Option<String>,
    /// Packages installed in the build root: the compiler, linker, make and any
    /// generator tools the distribution provides. Never target libraries.
    pub packages: Vec<String>,
    /// Extra packages appended to the sysroot, in addition to
    /// `packages.install`. Usually headers and development files that the build
    /// needs but the runtime closure does not.
    #[serde(default)]
    pub sysroot_packages: Vec<String>,
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
    let profile: Profile = yaml::parse_strict(text, codes::ROOT_006)?;
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
            rpmx_language::plan::normalize_dist_tag(tag)
                .map_err(|error| Error::new(codes::ROOT_006, error.message))?;
        }
        if self.archs.is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "profile must declare at least one architecture",
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
            if !valid_repository_id(&repository.id) {
                return Err(Error::new(
                    codes::ROOT_006,
                    format!(
                        "repository id '{}' must contain only letters, digits, '.', '_', '-' and '+'",
                        repository.id
                    ),
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
            if let Some(auth) = &repository.auth {
                validate_auth(&repository.id, auth)?;
            }
            let (kind, url) = repository.location();
            if has_url_userinfo(url) {
                return Err(Error::new(
                    codes::ROOT_006,
                    format!(
                        "repository '{}' embeds credentials in its {kind} URL; use the auth block instead",
                        repository.id
                    ),
                ));
            }
            for (field, value) in [
                ("baseurl", repository.baseurl.as_deref()),
                ("metalink", repository.metalink.as_deref()),
                ("mirrorlist", repository.mirrorlist.as_deref()),
                ("gpgkey", repository.gpgkey.as_deref()),
            ] {
                if let Some(value) = value {
                    validate_config_value(&repository.id, field, value)?;
                }
            }
        }
        if self.packages.install.is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "profile must install at least one package",
            ));
        }
        self.validate_cross()?;
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
        validate_install_path("paths.prefix", &self.paths.prefix)?;
        validate_install_path("paths.libdir", &self.paths.libdir)?;
        if self.paths.libdir != self.paths.prefix
            && !self
                .paths
                .libdir
                .starts_with(&format!("{}/", self.paths.prefix))
        {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "profile paths.libdir '{}' must be inside paths.prefix '{}'",
                    self.paths.libdir, self.paths.prefix
                ),
            ));
        }
        Ok(())
    }

    pub fn arch(&self) -> &'static str {
        // A profile is derived for one target before it reaches the resolver.
        debug_assert_eq!(self.archs.len(), 1, "resolve a profile for one target");
        normalize_arch(&self.archs[0]).expect("validated profile arch")
    }

    /// Derive the single-target profile used to resolve one root.
    ///
    /// `archs` is an allowed set. A profile may name several targets; a root is
    /// always resolved for exactly one. A requested target outside the set is
    /// an error, not a silent first-entry selection.
    pub fn for_target(&self, arch: &str) -> Result<Profile> {
        let target = normalize_arch(arch)?;
        let mut profile = self.clone();
        if profile.archs.is_empty() {
            profile.archs = vec![target.to_string()];
            return Ok(profile);
        }
        let allowed: Vec<&'static str> = profile
            .archs
            .iter()
            .map(|arch| normalize_arch(arch))
            .collect::<Result<_>>()?;
        if !allowed.contains(&target) {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "profile '{}' allows {}, but the requested architecture is {target}",
                    profile.name("profile"),
                    allowed.join(", ")
                ),
            ));
        }
        profile.archs = vec![target.to_string()];
        Ok(profile)
    }

    /// The cross toolchain, if this profile builds for a foreign target.
    pub fn cross(&self) -> Option<&Cross> {
        self.cross.as_ref()
    }

    /// The GNU triple naming the cross toolchain commands.
    ///
    /// Fails when the profile does not cross-compile, so callers cannot use a
    /// triple without a build root that provides those commands.
    pub fn triple(&self) -> Result<&str> {
        let cross = self
            .cross
            .as_ref()
            .ok_or_else(|| Error::new(codes::ROOT_006, "profile is not cross-compiling"))?;
        match cross.triple.as_deref() {
            Some(triple) => Ok(triple),
            None => Ok(rpmx_base::arch::Arch::parse(self.arch())?.triple()),
        }
    }

    /// The profile that resolves the host-architecture build root.
    ///
    /// It holds `cross.packages` and no `cross` block, so resolving it is an
    /// ordinary native root resolution.
    pub fn build_root_profile(&self) -> Result<Profile> {
        let cross = self
            .cross
            .as_ref()
            .ok_or_else(|| Error::new(codes::ROOT_006, "profile is not cross-compiling"))?;
        let host = match &cross.host {
            Some(host) => normalize_arch(host)?,
            None => rpmx_base::arch::Arch::current()?.as_str(),
        };
        let mut profile = self.clone();
        profile.name = Some(format!("{}-build", self.name("profile")));
        profile.archs = vec![host.to_string()];
        profile.packages.install = cross.packages.clone();
        profile.packages.reinstall.clear();
        profile.cross = None;
        Ok(profile)
    }

    /// The profile that resolves the target sysroot.
    ///
    /// It is `packages.install` plus the extra `-devel` packages, with no
    /// `cross` block so it resolves like any target root.
    pub fn sysroot_profile(&self) -> Result<Profile> {
        let cross = self
            .cross
            .as_ref()
            .ok_or_else(|| Error::new(codes::ROOT_006, "profile is not cross-compiling"))?;
        let mut packages: std::collections::BTreeSet<String> =
            self.packages.install.iter().cloned().collect();
        packages.extend(cross.sysroot_packages.iter().cloned());
        let mut profile = self.clone();
        profile.name = Some(format!("{}-sysroot", self.name("profile")));
        profile.archs = vec![self.arch().to_string()];
        profile.packages.install = packages.into_iter().collect();
        profile.packages.reinstall.clear();
        profile.cross = None;
        Ok(profile)
    }

    fn validate_cross(&self) -> Result<()> {
        let Some(cross) = &self.cross else {
            return Ok(());
        };
        if cross.packages.is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "cross.packages must list the build-root packages: the compiler, linker and make",
            ));
        }
        for package in cross.packages.iter().chain(cross.sysroot_packages.iter()) {
            if package.trim().is_empty() {
                return Err(Error::new(
                    codes::ROOT_006,
                    "cross package names must not be empty",
                ));
            }
        }
        if let Some(triple) = &cross.triple {
            if triple.trim().is_empty() || triple.contains(char::is_whitespace) {
                return Err(Error::new(
                    codes::ROOT_006,
                    "cross.triple must be a GNU triple such as aarch64-linux-gnu",
                ));
            }
        }
        let native = rpmx_base::arch::Arch::current()?;
        let host = match &cross.host {
            Some(host) => normalize_arch(host)?,
            None => native.as_str(),
        };
        if host != native.as_str() {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "cross build root is {host}, but rpmx is running on {native}; \
                     only a native build root is supported"
                ),
            ));
        }
        for arch in &self.archs {
            if normalize_arch(arch)? == host {
                return Err(Error::new(
                    codes::ROOT_006,
                    format!(
                        "cross target {host} equals the build-root architecture; \
                         remove the cross block and build natively"
                    ),
                ));
            }
        }
        Ok(())
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
            .map(rpmx_language::plan::normalize_dist_tag)
            .transpose()
    }
}

pub fn default_sbom() -> bool {
    true
}

pub fn default_prefix() -> String {
    rpmx_base::guest::PREFIX.to_string()
}

pub fn default_libdir() -> String {
    rpmx_base::guest::LIBDIR.to_string()
}

fn validate_install_path(field: &str, value: &str) -> Result<()> {
    let workspace = rpmx_base::guest::BUILD;
    let stage = rpmx_base::guest::STAGE;
    let invalid = value.is_empty()
        || value == "/"
        || !value.starts_with('/')
        || value.ends_with('/')
        || value
            .split('/')
            .any(|component| component == "." || component == "..")
        || value == workspace
        || value.starts_with(&format!("{workspace}/"))
        || value == stage
        || value.starts_with(&format!("{stage}/"));
    if invalid {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "profile {field} must be an absolute path outside the build workspace and stage, \
                 without a trailing slash, found '{value}'"
            ),
        ));
    }
    Ok(())
}

fn validate_auth(repository: &str, auth: &RepositoryAuth) -> Result<()> {
    if auth.is_empty() {
        return Err(Error::new(
            codes::ROOT_006,
            format!("repository '{repository}' has an empty auth block"),
        ));
    }
    for (field, value) in [
        ("auth.ca", auth.ca.as_deref()),
        ("auth.cert", auth.cert.as_deref()),
        ("auth.key", auth.key.as_deref()),
        ("auth.username", auth.username.as_deref()),
        ("auth.password", auth.password.as_deref()),
        ("auth.proxy", auth.proxy.as_deref()),
        ("auth.proxy-username", auth.proxy_username.as_deref()),
        ("auth.proxy-password", auth.proxy_password.as_deref()),
    ] {
        if let Some(value) = value {
            validate_config_value(repository, field, value)?;
        }
    }
    if auth.cert.is_some() != auth.key.is_some() {
        return Err(Error::new(
            codes::ROOT_006,
            format!("repository '{repository}' must set auth.cert and auth.key together"),
        ));
    }
    if auth.username.is_some() != auth.password.is_some() {
        return Err(Error::new(
            codes::ROOT_006,
            format!("repository '{repository}' must set auth.username and auth.password together"),
        ));
    }
    if auth.proxy_username.is_some() != auth.proxy_password.is_some() {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "repository '{repository}' must set auth.proxy-username and auth.proxy-password together"
            ),
        ));
    }
    if auth.proxy_password.is_some() && auth.proxy.is_none() {
        return Err(Error::new(
            codes::ROOT_006,
            format!("repository '{repository}' sets proxy credentials without auth.proxy"),
        ));
    }
    if let Some(proxy) = &auth.proxy {
        if proxy.contains('@') {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "repository '{repository}' embeds credentials in auth.proxy; use auth.proxy-username and auth.proxy-password instead"
                ),
            ));
        }
    }
    for (field, value) in [
        ("auth.password", auth.password.as_deref()),
        ("auth.proxy-password", auth.proxy_password.as_deref()),
    ] {
        if let Some(value) = value {
            if !is_secret_reference(value) {
                return Err(Error::new(
                    codes::ROOT_006,
                    format!(
                        "repository '{repository}' {field} must be an env:NAME reference or an absolute file path"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn is_secret_reference(value: &str) -> bool {
    if value.starts_with("env:") {
        return true;
    }
    let path = value.strip_prefix("file://").unwrap_or(value);
    std::path::Path::new(path).is_absolute()
}

pub(crate) fn has_control_characters(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn validate_config_value(repository: &str, field: &str, value: &str) -> Result<()> {
    if has_control_characters(value) {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "repository '{repository}' {field} contains a control character; repository values must be one line"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn has_url_userinfo(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    authority.contains('@')
}

/// Repository ids become file names under `repos.d`.
fn valid_repository_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '+')
        })
}

/// Normalize an architecture name to the RPM spelling used in NEVRAs.
pub fn normalize_arch(arch: &str) -> Result<&'static str> {
    rpmx_base::arch::Arch::parse(arch)
        .map(rpmx_base::arch::Arch::as_str)
        .map_err(|error| Error::new(codes::ROOT_009, error.message))
}

#[cfg(test)]
mod tests {
    use super::parse_profile;
    use rpmx_base::error::codes;

    fn profile(id: &str) -> String {
        format!(
            r#"
document: rpmx-profile
version: 1
releasever: "44"
archs: [x86_64]
repositories:
  - id: "{id}"
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
"#
        )
    }

    #[test]
    fn repository_ids_cannot_escape_the_repository_directory() {
        for id in ["../victim", "/tmp/victim", "a/b", "a\\b"] {
            let error = parse_profile(&profile(id)).expect_err(id);
            assert_eq!(error.code, codes::ROOT_006, "{id}");
        }
    }

    #[test]
    fn ordinary_repository_ids_are_accepted() {
        parse_profile(&profile("fedora-44_updates.1")).expect("valid id");
    }

    #[test]
    fn a_profile_allows_several_targets_and_derives_one() {
        let text = profile("base").replace("archs: [x86_64]", "archs: [x86_64, aarch64]");
        let profile = parse_profile(&text).expect("valid profile");

        let x86 = profile.for_target("amd64").expect("x86 target");
        assert_eq!(x86.archs, vec!["x86_64".to_string()]);
        let arm = profile.for_target("arm64").expect("arm target");
        assert_eq!(arm.archs, vec!["aarch64".to_string()]);
    }

    #[test]
    fn a_target_outside_the_profile_set_is_rejected() {
        let profile = parse_profile(&profile("base")).expect("valid profile");
        let error = profile.for_target("aarch64").expect_err("not allowed");
        assert_eq!(error.code, codes::ROOT_006);
    }

    #[test]
    fn paths_default_to_the_guest_layout() {
        let profile = parse_profile(&profile("base")).expect("valid profile");
        assert_eq!(profile.paths.prefix, "/usr");
        assert_eq!(profile.paths.libdir, "/usr/lib64");
    }

    #[test]
    fn paths_are_read_from_the_profile() {
        let text = profile("base").replace(
            "packages:",
            "paths:\n  prefix: /opt\n  libdir: /opt/lib\npackages:",
        );
        let profile = parse_profile(&text).expect("valid profile");
        assert_eq!(profile.paths.prefix, "/opt");
        assert_eq!(profile.paths.libdir, "/opt/lib");
    }

    #[test]
    fn invalid_paths_are_rejected() {
        for (prefix, libdir) in [
            ("relative", "/usr/lib64"),
            ("/usr/", "/usr/lib64"),
            ("/usr", "/opt/lib"),
            ("/", "/lib"),
            ("/tmp/stage", "/tmp/stage/lib"),
            ("/usr/../etc", "/usr/etc"),
        ] {
            let text = profile("base").replace(
                "packages:",
                &format!("paths:\n  prefix: \"{prefix}\"\n  libdir: \"{libdir}\"\npackages:"),
            );
            let error =
                parse_profile(&text).expect_err(&format!("prefix={prefix} libdir={libdir}"));
            assert_eq!(
                error.code,
                codes::ROOT_006,
                "prefix={prefix} libdir={libdir}"
            );
        }
    }

    fn cross_profile(target: &str, cross: &str) -> String {
        format!(
            r#"
document: rpmx-profile
version: 1
releasever: "44"
archs: [{target}]
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [bash, zlib-devel]
cross:
{cross}
"#
        )
    }

    fn other_arch() -> &'static str {
        if rpmx_base::arch::Arch::current().expect("host") == rpmx_base::arch::Arch::X86_64 {
            "aarch64"
        } else {
            "x86_64"
        }
    }

    #[test]
    fn a_cross_profile_splits_the_toolchain_from_the_sysroot() {
        let target = other_arch();
        let text = cross_profile(
            target,
            "  packages: [toolchain-cc, toolchain-binutils]\n  sysroot_packages: [libc-devel]",
        );
        let profile = parse_profile(&text).expect("valid cross profile");

        assert_eq!(
            profile.triple().expect("triple"),
            rpmx_base::arch::Arch::parse(target).unwrap().triple()
        );

        let build = profile.build_root_profile().expect("build root");
        let native = rpmx_base::arch::Arch::current().unwrap();
        assert_eq!(build.archs, vec![native.as_str().to_string()]);
        assert_eq!(
            build.packages.install,
            vec!["toolchain-cc".to_string(), "toolchain-binutils".to_string()]
        );
        assert!(build.cross.is_none(), "derived profiles must not re-cross");

        let sysroot = profile.sysroot_profile().expect("sysroot");
        assert_eq!(sysroot.archs, vec![target.to_string()]);
        assert_eq!(
            sysroot.packages.install,
            vec![
                "bash".to_string(),
                "libc-devel".to_string(),
                "zlib-devel".to_string()
            ]
        );
        assert!(sysroot.cross.is_none());
    }

    #[test]
    fn a_cross_triple_can_be_overridden() {
        let target = other_arch();
        let text = cross_profile(
            target,
            "  triple: \"aarch64-redhat-linux-gnu\"\n  packages: [toolchain-cc]",
        );
        let profile = parse_profile(&text).expect("valid cross profile");
        assert_eq!(profile.triple().unwrap(), "aarch64-redhat-linux-gnu");
    }

    #[test]
    fn a_cross_build_root_must_be_the_native_architecture() {
        let target = other_arch();
        let text = cross_profile(
            target,
            &format!("  host: {target}\n  packages: [toolchain-cc]"),
        );
        let error = parse_profile(&text).expect_err("foreign build root");
        assert_eq!(error.code, codes::ROOT_006);
        assert!(error.message.contains("running on"), "{}", error.message);
    }

    #[test]
    fn a_cross_target_must_differ_from_the_build_root() {
        let native = rpmx_base::arch::Arch::current().unwrap().as_str();
        let text = cross_profile(native, "  packages: [toolchain-cc]");
        let error = parse_profile(&text).expect_err("same architecture");
        assert_eq!(error.code, codes::ROOT_006);
        assert!(
            error.message.contains("build natively"),
            "{}",
            error.message
        );
    }

    #[test]
    fn cross_requires_the_build_root_packages() {
        let target = other_arch();
        let text = cross_profile(target, "  packages: []");
        let error = parse_profile(&text).expect_err("no toolchain");
        assert_eq!(error.code, codes::ROOT_006);
        assert!(
            error.message.contains("cross.packages"),
            "{}",
            error.message
        );
    }
}
