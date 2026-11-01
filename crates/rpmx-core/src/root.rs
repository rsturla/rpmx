use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::cas;
use crate::error::{codes, Error, Result};
use crate::hash::sha512_hex;
use crate::profile::{normalize_arch, Profile, Repository, ResolveOptions};

pub const DOCUMENT: &str = "rpmx-root-lock";
pub const VERSION: u32 = 1;

/// A resolved build root: repositories, an exact package closure and the
/// builder that produced it.
///
/// The `repositories` and `packages` sections use the libpkgmanifest
/// `rpm-package-manifest` vocabulary. The document header and integrity fields
/// are rpmx's own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootLock {
    pub document: String,
    pub version: u32,
    pub profile: String,
    pub archs: Vec<String>,
    pub releasever: String,
    /// Distribution tag recorded from the profile. It is provenance, not
    /// closure content, so it is deliberately outside `closure_digest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist_tag: Option<String>,
    #[serde(default)]
    pub options: ResolveOptions,
    #[serde(default)]
    pub debuginfo: bool,
    #[serde(default = "crate::profile::default_sbom")]
    pub sbom: bool,
    pub repositories: Vec<Repository>,
    pub packages: BTreeMap<String, Vec<LockedPackage>>,
    pub closure_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builder: Option<LockedBuilder>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedBuilder {
    pub image: String,
    /// Manifest digest for the lock's target architecture, not the multi-arch
    /// index digest.
    pub image_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub name: String,
    pub repo_id: String,
    pub evr: String,
    pub checksum: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub srpm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// The reproducible input: repositories plus the exact package closure.
///
/// Volatile fields (`root_digest`, builder image, `location`) are deliberately
/// excluded so two resolutions of the same inputs agree.
pub fn closure_digest(
    releasever: &str,
    archs: &[String],
    repositories: &[Repository],
    packages: &BTreeMap<String, Vec<LockedPackage>>,
) -> String {
    let mut material = String::from("rpmx-root-lock/1\n");
    material.push_str(&format!("releasever:{releasever}\n"));

    let mut sorted_archs = archs.to_vec();
    sorted_archs.sort();
    for arch in sorted_archs {
        material.push_str(&format!("arch:{arch}\n"));
    }

    let mut sorted_repos = repositories.to_vec();
    sorted_repos.sort_by(|left, right| left.id.cmp(&right.id));
    for repository in sorted_repos {
        let (kind, url) = repository.location();
        material.push_str(&format!("repo:{}:{kind}:{url}\n", repository.id));
    }

    for (arch, entries) in packages {
        let mut sorted = entries.to_vec();
        sorted.sort_by(|left, right| {
            (&left.name, &left.evr, &left.checksum).cmp(&(&right.name, &right.evr, &right.checksum))
        });
        for package in sorted {
            material.push_str(&format!(
                "pkg:{arch}:{}:{}:{}:{}:{}:{}:{}\n",
                package.name,
                package.evr,
                package.repo_id,
                package.checksum,
                package.size,
                package.srpm.as_deref().unwrap_or(""),
                package.module.as_deref().unwrap_or(""),
            ));
        }
    }

    sha512_hex(material.as_bytes())
}

pub fn resolve_profile(
    profile: &Profile,
    profile_name: &str,
    work: &Path,
    cache: &Path,
) -> Result<RootLock> {
    let arch = profile.arch();
    let image = profile.builder_image();
    let image_digest = image_digest_for_arch(&image, profile.arch())?;

    prepare_work(work)?;
    write_repositories(work, &profile.repositories)?;

    run_dnf(
        &image,
        work,
        dnf_base_args(profile)
            .into_iter()
            .chain([
                "download".to_string(),
                "--resolve".to_string(),
                "--destdir=/work/rpms".to_string(),
            ])
            .chain(profile.packages.install.iter().cloned())
            .collect(),
    )?;

    let downloaded = read_downloaded(&work.join("rpms"), cache)?;
    if downloaded.is_empty() {
        return Err(Error::new(
            codes::ROOT_008,
            "the resolver downloaded no packages",
        ));
    }
    for package in &downloaded {
        if package.arch != arch && package.arch != "noarch" {
            return Err(Error::new(
                codes::ROOT_009,
                format!(
                    "resolver pulled {}-{} for a {arch} root; multiarch is not supported",
                    package.name, package.arch
                ),
            ));
        }
    }

    let specs = downloaded.iter().map(Downloaded::spec).collect::<Vec<_>>();
    let metadata = query_repositories(&image, work, profile, &specs)?;

    let mut packages: BTreeMap<String, Vec<LockedPackage>> = BTreeMap::new();
    for package in downloaded {
        let found = metadata.get(&package.key()).ok_or_else(|| {
            Error::new(
                codes::ROOT_009,
                format!(
                    "resolved package {} is not present in any configured repository",
                    package.spec()
                ),
            )
        })?;
        packages
            .entry(package.arch.clone())
            .or_default()
            .push(LockedPackage {
                name: package.name.clone(),
                repo_id: found.repo_id.clone(),
                evr: package.evr(),
                checksum: package.digest.clone(),
                size: package.size,
                srpm: found.srpm.clone().or(package.srpm.clone()),
                module: None,
                location: None,
            });
    }
    for entries in packages.values_mut() {
        entries.sort_by(|left, right| (&left.name, &left.evr).cmp(&(&right.name, &right.evr)));
    }

    let archs = vec![arch.to_string()];
    let closure = closure_digest(
        &profile.releasever,
        &archs,
        &profile.repositories,
        &packages,
    );

    Ok(RootLock {
        document: DOCUMENT.to_string(),
        version: VERSION,
        profile: profile_name.to_string(),
        archs,
        releasever: profile.releasever.clone(),
        dist_tag: profile.dist_tag()?,
        options: profile.options.clone(),
        debuginfo: profile.debuginfo,
        sbom: profile.sbom,
        repositories: profile.repositories.clone(),
        packages,
        closure_digest: closure,
        root_digest: None,
        builder: Some(LockedBuilder {
            image,
            image_digest,
        }),
    })
}

fn prepare_work(work: &Path) -> Result<()> {
    // The RPMs and the empty installroot are outputs of this run. A reused work
    // directory must not feed packages from an earlier resolution into the
    // closure. The metadata cache is safe to keep: dnf revalidates it.
    for directory in ["rpms", "root"] {
        let path = work.join(directory);
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(|error| {
                Error::new(
                    codes::ROOT_004,
                    format!("cannot clear {}: {error}", path.display()),
                )
            })?;
        }
    }
    for directory in ["repos.d", "rpms", "root", "cache", "persist"] {
        std::fs::create_dir_all(work.join(directory)).map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot create resolver directory {}: {error}", directory),
            )
        })?;
    }
    Ok(())
}

fn write_repositories(work: &Path, repositories: &[Repository]) -> Result<()> {
    for repository in repositories {
        let (kind, url) = repository.location();
        // Fedora dropped signed repository metadata, so only package
        // signatures are checked. Metadata pinning is an open question.
        let mut text = format!(
            "[{}]\nname={}\n{kind}={url}\nenabled=1\ngpgcheck={}\nrepo_gpgcheck=0\n",
            repository.id,
            repository.id,
            u8::from(repository.gpgcheck()),
        );
        if let Some(key) = &repository.gpgkey {
            text.push_str(&format!("gpgkey={key}\n"));
        }
        let path = work.join("repos.d").join(format!("{}.repo", repository.id));
        std::fs::write(&path, text).map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot write repository file {}: {error}", path.display()),
            )
        })?;
    }
    Ok(())
}

fn dnf_base_args(profile: &Profile) -> Vec<String> {
    let mut args = vec![
        "dnf".to_string(),
        "--installroot=/root".to_string(),
        "--config=/etc/dnf/dnf.conf".to_string(),
        "--setopt=reposdir=/work/repos.d".to_string(),
        format!("--releasever={}", profile.releasever),
        format!("--forcearch={}", profile.arch()),
        format!(
            "--setopt=install_weak_deps={}",
            u8::from(profile.options.weak_deps)
        ),
        "--setopt=cachedir=/work/cache".to_string(),
        "--setopt=persistdir=/work/persist".to_string(),
    ];
    if !profile.options.docs {
        args.push("--setopt=tsflags=nodocs".to_string());
    }
    // Native roots only. Without this, dnf resolves the i686 companions of
    // multilib-capable packages into an otherwise x86_64 root.
    if profile.arch() == "x86_64" {
        args.push("--exclude=*.i686".to_string());
    }
    args
}

fn run_dnf(image: &str, work: &Path, args: Vec<String>) -> Result<()> {
    let output = podman_run(image, work, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(Error::new(
            codes::ROOT_007,
            format!("dnf resolution failed:\n{tail}"),
        ));
    }
    Ok(())
}

fn podman_run(image: &str, work: &Path, args: &[String]) -> Result<std::process::Output> {
    let mut process = Command::new("podman");
    process
        .arg("run")
        .arg("--rm")
        .arg("--pull=missing")
        .arg("--arch")
        .arg(crate::arch::Arch::current()?.to_string())
        .arg("-v")
        .arg(format!("{}:/work:Z", work.display()))
        .arg("-w")
        .arg("/work");
    for variable in [
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ] {
        if let Ok(value) = std::env::var(variable) {
            process.arg("-e").arg(format!("{variable}={value}"));
        }
    }
    process.arg(image).args(args);
    process.output().map_err(|error| {
        Error::new(
            codes::ROOT_001,
            format!("cannot run the resolver container: {error}"),
        )
    })
}

fn image_digest_for_arch(image: &str, arch: &str) -> Result<String> {
    let target = crate::arch::Arch::parse(arch)?;
    let oci_arch = match target {
        crate::arch::Arch::X86_64 => "amd64",
        crate::arch::Arch::Aarch64 => "arm64",
        crate::arch::Arch::Ppc64le => "ppc64le",
        crate::arch::Arch::S390x => "s390x",
        crate::arch::Arch::I686 => "386",
    };
    if let Ok(output) = Command::new("podman")
        .args(["manifest", "inspect", image])
        .output()
    {
        if output.status.success() {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                if let Some(manifests) = value.get("manifests").and_then(|value| value.as_array()) {
                    for manifest in manifests {
                        let platform = manifest.get("platform");
                        let architecture = platform
                            .and_then(|platform| platform.get("architecture"))
                            .and_then(|value| value.as_str());
                        let os = platform
                            .and_then(|platform| platform.get("os"))
                            .and_then(|value| value.as_str());
                        if architecture == Some(oci_arch) && os == Some("linux") {
                            if let Some(digest) =
                                manifest.get("digest").and_then(|value| value.as_str())
                            {
                                return Ok(digest.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    // A local image has no manifest index. Accept it only when its recorded
    // architecture matches the requested one.
    let digest = ensure_image(image)?;
    let actual_arch = inspect_image(image, "{{.Architecture}}")?;
    let expected = match target {
        crate::arch::Arch::X86_64 => "amd64",
        crate::arch::Arch::Aarch64 => "arm64",
        crate::arch::Arch::Ppc64le => "ppc64le",
        crate::arch::Arch::S390x => "s390x",
        crate::arch::Arch::I686 => "386",
    };
    if actual_arch != expected {
        return Err(Error::new(
            codes::ROOT_002,
            format!("builder image {image} is {actual_arch}, but a {expected} image is required"),
        ));
    }
    Ok(digest)
}

fn ensure_image(image: &str) -> Result<String> {
    match inspect_image(image, "{{.Digest}}") {
        Ok(digest) if !digest.is_empty() => Ok(digest),
        _ => {
            let status = Command::new("podman")
                .args(["pull", "--quiet", image])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|error| {
                    Error::new(codes::ROOT_001, format!("cannot run podman: {error}"))
                })?;
            if !status.success() {
                return Err(Error::new(
                    codes::ROOT_002,
                    format!("cannot pull builder image '{image}'"),
                ));
            }
            let digest = inspect_image(image, "{{.Digest}}")?;
            if digest.is_empty() {
                return Err(Error::new(
                    codes::ROOT_002,
                    format!("builder image '{image}' has no digest"),
                ));
            }
            Ok(digest)
        }
    }
}

fn inspect_image(image: &str, format: &str) -> Result<String> {
    let output = Command::new("podman")
        .args(["image", "inspect", "--format", format, image])
        .output()
        .map_err(|error| {
            Error::new(
                codes::ROOT_001,
                format!("cannot inspect image '{image}': {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::ROOT_002,
            format!(
                "cannot inspect image '{image}': {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

struct Downloaded {
    name: String,
    epoch: u32,
    version: String,
    release: String,
    arch: String,
    digest: String,
    size: u64,
    srpm: Option<String>,
}

impl Downloaded {
    fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.name, self.epoch, self.version, self.release, self.arch
        )
    }

    fn evr(&self) -> String {
        if self.epoch == 0 {
            format!("{}-{}", self.version, self.release)
        } else {
            format!("{}:{}-{}", self.epoch, self.version, self.release)
        }
    }

    fn spec(&self) -> String {
        format!("{}-{}.{}", self.name, self.evr(), self.arch)
    }
}

fn read_downloaded(directory: &Path, cache: &Path) -> Result<Vec<Downloaded>> {
    let mut entries = std::fs::read_dir(directory)
        .map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            Error::new(
                codes::ROOT_004,
                format!("cannot read {}: {error}", directory.display()),
            )
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut packages = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("rpm") {
            continue;
        }
        let package = rpm::Package::open(&path).map_err(|error| {
            Error::new(
                codes::ROOT_010,
                format!("cannot read downloaded package {}: {error}", path.display()),
            )
        })?;
        let bytes = std::fs::read(&path).map_err(|error| {
            Error::new(
                codes::ROOT_010,
                format!("cannot read downloaded package {}: {error}", path.display()),
            )
        })?;
        let header = |error: rpm::Error| {
            Error::new(
                codes::ROOT_010,
                format!("cannot read package header in {}: {error}", path.display()),
            )
        };
        let digest = cas::store(cache, &bytes)?;
        packages.push(Downloaded {
            name: package.metadata.get_name().map_err(header)?.to_string(),
            epoch: package.metadata.get_epoch().unwrap_or(0),
            version: package.metadata.get_version().map_err(header)?.to_string(),
            release: package.metadata.get_release().map_err(header)?.to_string(),
            arch: package.metadata.get_arch().map_err(header)?.to_string(),
            digest,
            size: bytes.len() as u64,
            srpm: package
                .metadata
                .get_source_rpm()
                .ok()
                .filter(|value| !value.is_empty() && *value != "(none)")
                .map(str::to_string),
        });
    }
    Ok(packages)
}

struct RepositoryMatch {
    repo_id: String,
    srpm: Option<String>,
}

fn query_repositories(
    image: &str,
    work: &Path,
    profile: &Profile,
    specs: &[String],
) -> Result<BTreeMap<String, RepositoryMatch>> {
    let mut args = dnf_base_args(profile);
    args.extend([
        "repoquery".to_string(),
        "--queryformat".to_string(),
        "%{name}|%{epoch}|%{version}|%{release}|%{arch}|%{repoid}|%{sourcerpm}\n".to_string(),
    ]);
    args.extend(specs.iter().cloned());

    let output = podman_run(image, work, &args)?;
    if !output.status.success() {
        return Err(Error::new(
            codes::ROOT_007,
            format!(
                "repository metadata query failed:\n{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    let mut found: BTreeMap<String, RepositoryMatch> = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields = line.split('|').collect::<Vec<_>>();
        if fields.len() != 7 {
            continue;
        }
        let key = format!(
            "{}|{}|{}|{}|{}",
            fields[0], fields[1], fields[2], fields[3], fields[4]
        );
        let srpm = fields[6].trim();
        let srpm = (!srpm.is_empty() && srpm != "(none)").then(|| srpm.to_string());
        let repo_id = fields[5].to_string();
        if let Some(existing) = found.get(&key) {
            if existing.repo_id != repo_id {
                return Err(Error::new(
                    codes::ROOT_009,
                    format!(
                        "package {key} is provided by both '{}' and '{repo_id}'; \
                         repository provenance is ambiguous",
                        existing.repo_id
                    ),
                ));
            }
            continue;
        }
        found.insert(key, RepositoryMatch { repo_id, srpm });
    }
    Ok(found)
}

/// Load a lock and check that it is a document this rpmx understands.
pub fn load_lock(path: &Path) -> Result<RootLock> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot read root lock {}: {error}", path.display()),
        )
    })?;
    let lock: RootLock = serde_yaml_ng::from_str(&text)
        .map_err(|error| Error::new(codes::ROOT_006, format!("invalid root lock: {error}")))?;
    if lock.document != DOCUMENT {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "root lock document must be '{DOCUMENT}', found '{}'",
                lock.document
            ),
        ));
    }
    if lock.version != VERSION {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "unsupported root lock version {}; this rpmx understands {VERSION}",
                lock.version
            ),
        ));
    }
    for arch in &lock.archs {
        normalize_arch(arch)?;
    }
    let expected = closure_digest(
        &lock.releasever,
        &lock.archs,
        &lock.repositories,
        &lock.packages,
    );
    if lock.closure_digest != expected {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "root lock closure digest mismatch: recorded {}, recomputed {expected}",
                lock.closure_digest
            ),
        ));
    }
    Ok(lock)
}

pub fn write_lock(path: &Path, lock: &RootLock) -> Result<()> {
    let text = serde_yaml_ng::to_string(lock)
        .map_err(|error| Error::new(codes::ROOT_004, error.to_string()))?;
    std::fs::write(path, text).map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}

pub struct AssembleOptions<'a> {
    pub cache: &'a Path,
    pub work: &'a Path,
    pub root: &'a Path,
}

/// Install a locked closure into an empty root, offline, and verify that the
/// installed package set matches the lock.
///
/// The root is left in `options.work/root`; the caller computes its digest and
/// records it in the lock.
pub fn assemble(lock: &RootLock, options: AssembleOptions<'_>) -> Result<std::path::PathBuf> {
    let lock_arch = lock
        .archs
        .first()
        .ok_or_else(|| Error::new(codes::ROOT_015, "lock declares no architecture"))?;
    let builder = lock.builder.as_ref().ok_or_else(|| {
        Error::new(
            codes::ROOT_015,
            "this lock has no builder image, so it cannot be assembled",
        )
    })?;
    let actual_digest = image_digest_for_arch(&builder.image, lock_arch)?;
    if actual_digest != builder.image_digest {
        return Err(Error::new(
            codes::ROOT_002,
            format!(
                "builder image '{}' has drifted: lock records {}, local image is {}",
                builder.image, builder.image_digest, actual_digest
            ),
        ));
    }
    let arch = lock
        .archs
        .first()
        .ok_or_else(|| Error::new(codes::ROOT_015, "lock declares no architecture"))?;
    let arch = normalize_arch(arch)?;
    let target = crate::arch::Arch::parse(arch)?;
    let host = crate::arch::Arch::current()?;
    if host != target && !crate::sandbox::emulation_available(target) {
        return Err(Error::new(
            codes::RUNNER_002,
            format!(
                "cannot assemble a {target} root on a {host} host; \
                 no qemu-user binfmt handler is available"
            ),
        ));
    }

    prepare_assembly(options.work, options.root)?;
    let rpms = options.work.join("rpms");

    let mut files = Vec::new();
    for (package_arch, entries) in &lock.packages {
        for package in entries {
            let filename = format!(
                "{}-{}.{package_arch}.rpm",
                package.name,
                package.evr.replace(':', "_")
            );
            let bytes = cas::load(options.cache, &package.checksum)?;
            std::fs::write(rpms.join(&filename), &bytes).map_err(|error| {
                Error::new(
                    codes::ROOT_012,
                    format!("cannot stage {}: {error}", filename),
                )
            })?;
            files.push(format!("/work/rpms/{filename}"));
        }
    }
    if files.is_empty() {
        return Err(Error::new(
            codes::ROOT_015,
            "the lock contains no packages to install",
        ));
    }
    std::fs::create_dir_all(options.root).map_err(|error| {
        Error::new(
            codes::ROOT_012,
            format!("cannot create {}: {error}", options.root.display()),
        )
    })?;
    // `setup` can be installed before `filesystem`; its sysusers trigger
    // creates /lib64 as a real directory, which then makes filesystem's
    // usrmerge symlink fail. Seed the layout before the transaction.
    create_usrmerge_layout(options.root)?;

    let mut args = vec![
        "dnf".to_string(),
        "-y".to_string(),
        "--installroot=/root".to_string(),
        "--config=/etc/dnf/dnf.conf".to_string(),
        "--setopt=reposdir=/work/empty-repos".to_string(),
        "--setopt=cachedir=/work/dnf-cache".to_string(),
        "--setopt=persistdir=/work/dnf-persist".to_string(),
        format!("--releasever={}", lock.releasever),
        format!("--forcearch={arch}"),
        format!(
            "--setopt=install_weak_deps={}",
            u8::from(lock.options.weak_deps)
        ),
    ];
    let mut tsflags = vec!["nocaps"];
    if !lock.options.docs {
        tsflags.insert(0, "nodocs");
    }
    args.push(format!("--setopt=tsflags={}", tsflags.join(",")));
    args.push("install".to_string());
    args.extend(files);

    let output = podman_run_assembly(&builder.image, options.work, options.root, target, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        // The root was empty before this run; remove the partial transaction so
        // the same command can be retried.
        let _ = remove_tree(options.root);
        return Err(Error::new(
            codes::ROOT_012,
            format!("offline install failed:\n{tail}"),
        ));
    }

    verify_installed(lock, options.root, &builder.image, target)?;
    Ok(options.root.to_path_buf())
}

fn prepare_assembly(work: &Path, root: &Path) -> Result<()> {
    if root.starts_with(work) {
        return Err(Error::new(
            codes::ROOT_012,
            format!(
                "the root {} must not be inside the scratch directory {}",
                root.display(),
                work.display()
            ),
        ));
    }
    let staging = work.join("rpms");
    if staging.exists() {
        remove_tree(&staging)?;
    }
    for directory in ["rpms", "empty-repos", "dnf-cache", "dnf-persist"] {
        std::fs::create_dir_all(work.join(directory)).map_err(|error| {
            Error::new(
                codes::ROOT_012,
                format!("cannot create assembly directory {directory}: {error}"),
            )
        })?;
    }
    if root.exists() {
        let empty = std::fs::read_dir(root)
            .map_err(|error| {
                Error::new(
                    codes::ROOT_012,
                    format!("cannot read {}: {error}", root.display()),
                )
            })?
            .next()
            .is_none();
        if !empty {
            return Err(Error::new(
                codes::ROOT_012,
                format!(
                    "root directory {} is not empty; refusing to assemble over it",
                    root.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Seed the usrmerge layout before a fresh rpm transaction. Fedora's
/// `filesystem` package creates these in `%pretrans`, but `setup` may be
/// ordered first and its sysusers trigger otherwise creates `/lib64` as a
/// real directory.
fn create_usrmerge_layout(root: &Path) -> Result<()> {
    for directory in [
        "usr/bin",
        "usr/lib",
        "usr/lib64",
        "usr/local/bin",
        "usr/lib/debug/usr",
    ] {
        std::fs::create_dir_all(root.join(directory)).map_err(|error| {
            Error::new(
                codes::ROOT_012,
                format!("cannot create {}: {error}", root.join(directory).display()),
            )
        })?;
    }
    for (link, target) in [
        ("bin", "usr/bin"),
        ("sbin", "usr/sbin"),
        ("lib", "usr/lib"),
        ("lib64", "usr/lib64"),
        ("usr/sbin", "bin"),
        ("usr/local/sbin", "bin"),
        ("usr/lib/debug/usr/sbin", "bin"),
        ("usr/lib/debug/bin", "usr/bin"),
        ("usr/lib/debug/lib", "usr/lib"),
        ("usr/lib/debug/lib64", "usr/lib64"),
        ("var/run", "../run"),
        ("var/lock", "../run/lock"),
    ] {
        let path = root.join(link);
        if path.exists() || path.symlink_metadata().is_ok() {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::new(
                    codes::ROOT_012,
                    format!("cannot create {}: {error}", parent.display()),
                )
            })?;
        }
        std::os::unix::fs::symlink(target, &path).map_err(|error| {
            Error::new(
                codes::ROOT_012,
                format!("cannot create {}: {error}", path.display()),
            )
        })?;
    }
    Ok(())
}

fn podman_run_assembly(
    image: &str,
    work: &Path,
    root: &Path,
    target: crate::arch::Arch,
    args: &[String],
) -> Result<std::process::Output> {
    let mut process = Command::new("podman");
    process
        .arg("run")
        .arg("--rm")
        .arg("--pull=missing")
        .arg("--arch")
        .arg(target.to_string());
    process
        .arg("--network=none")
        .arg("--read-only")
        .arg("--tmpfs")
        .arg("/tmp:rw,size=512m")
        .arg("--pids-limit=1024")
        .arg("--cap-drop=all")
        .arg("--cap-add=CHOWN,DAC_OVERRIDE,FOWNER,FSETID,SETGID,SETUID,SYS_CHROOT,SYS_ADMIN,MKNOD")
        .arg("--security-opt=no-new-privileges")
        .arg("--security-opt=seccomp=unconfined")
        .arg("-v")
        .arg(format!("{}:/work:Z", work.display()))
        .arg("-v")
        .arg(format!("{}:/root:Z", root.display()))
        .arg("-w")
        .arg("/work");
    process.arg(image).args(args);
    process.output().map_err(|error| {
        Error::new(
            codes::ROOT_012,
            format!("cannot run the assembly container: {error}"),
        )
    })
}

fn verify_installed(
    lock: &RootLock,
    root: &Path,
    image: &str,
    target: crate::arch::Arch,
) -> Result<()> {
    let mut expected = std::collections::BTreeSet::new();
    for (package_arch, entries) in &lock.packages {
        for package in entries {
            let (epoch, version, release) = evr_parts(&package.evr)?;
            expected.insert(format!(
                "{}|{epoch}|{version}|{release}|{package_arch}",
                package.name
            ));
        }
    }

    let mut process = Command::new("podman");
    process
        .arg("run")
        .arg("--rm")
        .arg("--pull=missing")
        .arg("--arch")
        .arg(target.to_string());
    process
        .arg("--network=none")
        .arg("--read-only")
        .arg("--cap-drop=all")
        .arg("--security-opt=no-new-privileges")
        .arg("-v")
        .arg(format!("{}:/root:Z", root.display()))
        .arg(image)
        .arg("rpm")
        .arg("--root")
        .arg("/root")
        .arg("-qa")
        .arg("--qf")
        .arg("%{NAME}|%{EPOCH}|%{VERSION}|%{RELEASE}|%{ARCH}\n");
    let output = process.output().map_err(|error| {
        Error::new(
            codes::ROOT_013,
            format!("cannot query the assembled root: {error}"),
        )
    })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::ROOT_013,
            format!(
                "cannot query the assembled root: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    let mut installed = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields = line.split('|').collect::<Vec<_>>();
        if fields.len() != 5 {
            continue;
        }
        let epoch = if fields[1] == "(none)" {
            "0"
        } else {
            fields[1]
        };
        installed.insert(format!(
            "{}|{epoch}|{}|{}|{}",
            fields[0], fields[2], fields[3], fields[4]
        ));
    }

    if installed != expected {
        let missing = expected
            .difference(&installed)
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        let extra = installed
            .difference(&expected)
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        return Err(Error::new(
            codes::ROOT_013,
            format!(
                "installed package set differs from the lock ({} expected, {} installed); missing: {}; extra: {}",
                expected.len(),
                installed.len(),
                missing.join(", "),
                extra.join(", ")
            ),
        ));
    }
    Ok(())
}

/// Remove a tree that may contain mode-000 files written by rpm.
///
/// Rootless podman maps container root to the calling user, but a directory
/// without write permission still cannot be emptied by its owner. Re-enter the
/// user namespace where the caller is root.
pub fn remove_tree(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(first) => {
            let status = Command::new("podman")
                .arg("unshare")
                .arg("--")
                .arg("rm")
                .arg("-rf")
                .arg(path)
                .status();
            match status {
                Ok(status) if status.success() => Ok(()),
                _ => Err(Error::new(
                    codes::ROOT_012,
                    format!("cannot remove {}: {first}", path.display()),
                )),
            }
        }
    }
}

fn evr_parts(evr: &str) -> Result<(String, String, String)> {
    let (epoch, rest) = match evr.split_once(':') {
        Some((epoch, rest)) => (epoch, rest),
        None => ("0", evr),
    };
    let (version, release) = rest.rsplit_once('-').ok_or_else(|| {
        Error::new(
            codes::ROOT_015,
            format!("package evr '{evr}' has no release separator"),
        )
    })?;
    if epoch.is_empty() || version.is_empty() || release.is_empty() {
        return Err(Error::new(
            codes::ROOT_015,
            format!("package evr '{evr}' is malformed"),
        ));
    }
    Ok((epoch.to_string(), version.to_string(), release.to_string()))
}
