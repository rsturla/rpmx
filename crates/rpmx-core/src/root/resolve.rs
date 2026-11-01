use std::collections::BTreeMap;
use std::path::Path;

use crate::cas;
use crate::error::{codes, Error, Result};
use crate::profile::{Profile, Repository};
use crate::root::container::{image_digest_for_arch, podman_run};
use crate::root::lock::{
    closure_digest, LockedBuilder, LockedPackage, RootLock, DOCUMENT, VERSION,
};

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
