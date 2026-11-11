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
    #[serde(default)]
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
    /// Architecture-specific overrides for the library directory.
    #[serde(
        default,
        rename = "libdir-by-arch",
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub libdir_by_arch: std::collections::BTreeMap<String, String>,
}

impl Paths {
    /// Select the target's override, falling back to the common directory.
    pub fn libdir_for(&self, arch: &str) -> &str {
        self.libdir_by_arch
            .get(arch)
            .map_or(&self.libdir, String::as_str)
    }
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            prefix: default_prefix(),
            libdir: default_libdir(),
            libdir_by_arch: Default::default(),
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
    /// Package names excluded from this repository. Bootstrap uses it to make
    /// the stage output win over an input repository with the same NEVRA.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Repository priority, lowest first. A repository without a priority
    /// keeps dnf's default, so a local repository can sit behind the published
    /// ones and still provide packages they do not carry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packages {
    #[serde(default)]
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
    /// Allow the build root and sysroot to share an architecture.
    ///
    /// This is the native bootstrap mode: a seed build root and a target
    /// sysroot can both be x86_64 while remaining separate roots. Without it,
    /// a same-architecture cross profile is rejected by `validate_cross` and
    /// `build_root_profile`.
    #[serde(default)]
    pub bootstrap: bool,
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
    /// Compiler flags every step sees. A toolchain quirk belongs here, in the
    /// distribution's profile, never in the executor.
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    /// Linker flags every step sees.
    #[serde(default)]
    pub ldflags: Vec<String>,
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
    load_profile_with(path, Profile::validate)
}

/// Load runtime repository policy without requiring or validating ignored seeds.
pub fn load_test_profile(path: &std::path::Path) -> Result<Profile> {
    load_profile_with(path, Profile::validate_for_tests)
}

fn load_profile_with(
    path: &std::path::Path,
    validate: fn(&Profile) -> Result<()>,
) -> Result<Profile> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::ROOT_006,
            format!("cannot read profile {}: {error}", path.display()),
        )
    })?;
    let mut profile: Profile = yaml::parse_strict(&text, codes::ROOT_006)?;
    validate(&profile)?;
    profile.resolve_local_paths(path)?;
    Ok(profile)
}

impl Profile {
    /// Resolve `file://` repository paths relative to the profile.
    ///
    /// A profile is often checked out with a local bootstrap repository next to
    /// it. Absolute host paths do not survive a CI checkout, so a relative
    /// `file://` path is anchored at the profile's directory and stored as the
    /// absolute path the resolver mounts.
    fn resolve_local_paths(&mut self, profile: &std::path::Path) -> Result<()> {
        let profile = std::fs::canonicalize(profile).map_err(|error| {
            Error::new(
                codes::ROOT_006,
                format!("cannot resolve profile {}: {error}", profile.display()),
            )
        })?;
        let directory = profile
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        for repository in &mut self.repositories {
            for (field, value) in [
                ("baseurl", &mut repository.baseurl),
                ("gpgkey", &mut repository.gpgkey),
            ] {
                if let Some(url) = value.as_deref() {
                    if let Some(relative) = relative_local_path(url) {
                        let joined = directory.join(relative);
                        // The repository may be produced by a bootstrap step
                        // after the profile is parsed (lint, plan, and the
                        // toolchain build all load it first). Resolve the path
                        // now, but leave the existence check to the resolver.
                        let resolved = std::fs::canonicalize(&joined)
                            .or_else(|_| std::path::absolute(&joined))
                            .map_err(|error| {
                                Error::new(
                                    codes::ROOT_006,
                                    format!(
                                        "repository '{}' {field} resolves to {}: {error}",
                                        repository.id,
                                        joined.display()
                                    ),
                                )
                            })?;
                        *value = Some(format!("file://{}", resolved.display()));
                    }
                }
            }
        }
        Ok(())
    }
}

fn relative_local_path(value: &str) -> Option<&std::path::Path> {
    let path = value.strip_prefix("file://")?;
    let path = std::path::Path::new(path);
    (!path.is_absolute()).then_some(path)
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        self.validate_policy()?;
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
        Ok(())
    }

    /// Test roots select their own package requests for each environment.
    pub fn validate_for_tests(&self) -> Result<()> {
        self.validate_policy()?;
        if self.cross.is_some() {
            return Err(Error::new(codes::TEST_004, "a test profile describes the target runtime and cannot contain cross configuration"));
        }
        Ok(())
    }

    fn validate_policy(&self) -> Result<()> {
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
            for package in &repository.exclude {
                if package.trim().is_empty()
                    || package.contains(',')
                    || package.chars().any(char::is_control)
                {
                    return Err(Error::new(
                        codes::ROOT_006,
                        format!(
                            "repository '{}' has an invalid exclude entry '{package}'; expected a \
                             package name without commas or control characters",
                            repository.id
                        ),
                    ));
                }
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
        if !self.modules.enable.is_empty() || !self.modules.disable.is_empty() {
            return Err(Error::new(
                codes::ROOT_006,
                "module streams are not supported yet",
            ));
        }
        validate_install_path("paths.prefix", &self.paths.prefix)?;
        for (arch, libdir) in &self.paths.libdir_by_arch {
            if normalize_arch(arch).ok() != Some(arch.as_str()) {
                return Err(Error::new(codes::ROOT_006, format!("paths.libdir-by-arch key '{arch}' must use its canonical architecture name")));
            }
            validate_install_path("paths.libdir-by-arch", libdir)?;
        }
        for libdir in std::iter::once(&self.paths.libdir).chain(self.paths.libdir_by_arch.values())
        {
            validate_install_path("paths.libdir", libdir)?;
            if libdir != &self.paths.prefix
                && !libdir.starts_with(&format!("{}/", self.paths.prefix))
            {
                return Err(Error::new(
                    codes::ROOT_006,
                    format!(
                        "profile paths.libdir '{libdir}' must be inside paths.prefix '{}'",
                        self.paths.prefix
                    ),
                ));
            }
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
        let native = rpmx_base::arch::Arch::current()?;
        if host != native.as_str() {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "cross build root is {host}, but rpmx is running on {native}; \
                     build on the {host} host or use a native profile"
                ),
            ));
        }
        if !cross.bootstrap {
            for arch in &self.archs {
                if normalize_arch(arch)? == host {
                    return Err(Error::new(
                        codes::ROOT_006,
                        format!(
                            "profile '{}' cross-builds for {}, so it cannot also build {host}; \
                             use a native profile for {host}",
                            self.name("profile"),
                            self.archs.join(", ")
                        ),
                    ));
                }
            }
        }
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
        for flag in cross
            .cflags
            .iter()
            .chain(cross.cxxflags.iter())
            .chain(cross.ldflags.iter())
        {
            if flag.trim().is_empty() {
                return Err(Error::new(
                    codes::ROOT_006,
                    "cross toolchain flags must not be empty",
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
        // A profile is portable data: it records which host its build root is
        // for, but loading it on another machine is allowed. Only deriving a
        // build root enforces that it is the current host and not the target.
        if let Some(host) = &cross.host {
            let host = normalize_arch(host)?;
            if !cross.bootstrap {
                for arch in &self.archs {
                    if normalize_arch(arch)? == host {
                        return Err(Error::new(
                            codes::ROOT_006,
                            format!(
                                "cross target {host} equals the build-root architecture; \
                                 set cross.bootstrap: true for a native bootstrap, or remove \
                                 the cross block and build natively"
                            ),
                        ));
                    }
                }
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
    use super::{load_profile, parse_profile};
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
    fn a_relative_local_repository_is_resolved_against_the_profile() {
        let directory = tempfile::tempdir().expect("tempdir");
        let profile_dir = directory.path().join("profiles");
        let repository = directory.path().join("toolchain");
        std::fs::create_dir_all(&profile_dir).expect("profile dir");
        std::fs::create_dir_all(&repository).expect("repository");
        let path = profile_dir.join("bootstrap.yaml");
        let text =
            profile("toolchain").replace("https://example.invalid/fedora", "file://../toolchain");
        std::fs::write(&path, text).expect("profile");

        let loaded = load_profile(&path).expect("profile");
        assert_eq!(
            loaded.repositories[0].baseurl,
            Some(format!(
                "file://{}",
                repository.canonicalize().unwrap().display()
            ))
        );
    }

    #[test]
    fn a_missing_relative_repository_is_left_for_the_resolver() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("bootstrap.yaml");
        let text =
            profile("toolchain").replace("https://example.invalid/fedora", "file://../missing");
        std::fs::write(&path, text).expect("profile");

        let loaded = load_profile(&path).expect("profile");
        let baseurl = loaded.repositories[0].baseurl.as_deref().expect("baseurl");
        assert!(baseurl.ends_with("/missing"), "{baseurl}");
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
    fn library_directory_can_vary_by_target() {
        let text = profile("base")
            .replace("archs: [x86_64]", "archs: [x86_64, aarch64]")
            .replace(
                "packages:",
                "paths:\n  libdir: /usr/lib64\n  libdir-by-arch:\n    aarch64: /usr/lib\npackages:",
            );
        let profile = parse_profile(&text).expect("valid profile");
        for (target, expected) in [("amd64", "/usr/lib64"), ("arm64", "/usr/lib")] {
            let selected = profile.for_target(target).expect("selected target");
            assert_eq!(selected.paths.libdir_for(selected.arch()), expected);
        }
    }

    #[test]
    fn architecture_library_directories_are_validated() {
        for entry in ["aarch64: /opt/lib", "arm64: /usr/lib", "unknown: /usr/lib"] {
            let text = profile("base").replace(
                "packages:",
                &format!("paths:\n  libdir-by-arch:\n    {entry}\npackages:"),
            );
            assert_eq!(parse_profile(&text).expect_err(entry).code, codes::ROOT_006);
        }
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
    fn a_cross_build_root_is_checked_when_it_is_derived() {
        let native = rpmx_base::arch::Arch::current().expect("host");
        let target = other_arch();
        // A third architecture: not this host, not the target.
        let foreign = ["s390x", "ppc64le", "aarch64", "x86_64", "i686"]
            .into_iter()
            .find(|arch| *arch != native.as_str() && *arch != target)
            .expect("a third architecture");
        let text = cross_profile(
            target,
            &format!("  host: {foreign}\n  packages: [toolchain-cc]"),
        );
        // A profile is portable data: loading it on another host is fine.
        let profile = parse_profile(&text).expect("a cross profile is valid data");
        // Deriving a build root for the wrong host is not.
        let error = profile
            .build_root_profile()
            .expect_err("a foreign build root cannot be assembled here");
        assert_eq!(error.code, codes::ROOT_006);
        assert!(error.message.contains("running on"), "{}", error.message);
        // The sysroot half stays usable, which is what a target machine
        // testing prebuilt packages needs.
        assert_eq!(
            profile.sysroot_profile().expect("sysroot").archs,
            vec![target.to_string()]
        );
    }

    #[test]
    fn a_cross_target_must_differ_from_the_build_root() {
        let native = rpmx_base::arch::Arch::current().unwrap().as_str();
        // Without an explicit host the check happens when the build root is
        // derived, because the host is only known then.
        let profile = parse_profile(&cross_profile(native, "  packages: [toolchain-cc]"))
            .expect("portable profile");
        let error = profile.build_root_profile().expect_err("same architecture");
        assert_eq!(error.code, codes::ROOT_006);
        assert!(
            error.message.contains("cannot also build"),
            "{}",
            error.message
        );

        // An explicit host is checked at parse time.
        let text = cross_profile(
            native,
            &format!("  host: {native}\n  packages: [toolchain-cc]"),
        );
        let error = parse_profile(&text).expect_err("explicit same host");
        assert_eq!(error.code, codes::ROOT_006);
    }

    #[test]
    fn a_native_bootstrap_may_share_the_build_root_architecture() {
        let native = rpmx_base::arch::Arch::current().unwrap().as_str();
        let text = cross_profile(
            native,
            &format!(
                "  host: {native}\n  bootstrap: true\n  packages: [toolchain-cc]\n  sysroot_packages: [libc-devel]"
            ),
        );
        let profile = parse_profile(&text).expect("native bootstrap profile");
        let cross = profile.cross().expect("cross block");
        assert!(cross.bootstrap);

        let build = profile.build_root_profile().expect("build root");
        let sysroot = profile.sysroot_profile().expect("sysroot");
        assert_eq!(build.archs, vec![native.to_string()]);
        assert_eq!(sysroot.archs, vec![native.to_string()]);
        assert_eq!(build.packages.install, vec!["toolchain-cc".to_string()]);
        assert!(sysroot.packages.install.contains(&"libc-devel".to_string()));
        assert!(!sysroot
            .packages
            .install
            .contains(&"toolchain-cc".to_string()));
        assert_ne!(
            build.name("build"),
            sysroot.name("sysroot"),
            "the two roots need distinct lock identities"
        );

        let target = profile.for_target(native).expect("target");
        assert!(target.cross().expect("cross").bootstrap);
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
