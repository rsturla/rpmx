use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::profile::{Profile, Repository};
use crate::root::container::{image_digest_for_arch, podman_run};
use crate::root::lock::{
    closure_digest, LockedBuilder, LockedPackage, PackageKey, RootLock, DOCUMENT, VERSION,
};
use rpmx_base::error::{codes, Error, Result};
use rpmx_store::cas;

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
    let local_repositories = write_repositories(work, &profile.repositories)?;

    run_dnf(
        &image,
        work,
        &local_repositories,
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
    let metadata = query_repositories(&image, work, profile, &specs, &local_repositories)?;

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

fn write_repositories(work: &Path, repositories: &[Repository]) -> Result<Vec<(PathBuf, String)>> {
    let mut mounts = Vec::new();
    for (index, repository) in repositories.iter().enumerate() {
        if repository.id.is_empty()
            || repository.id == "."
            || repository.id == ".."
            || repository.id.contains('/')
            || repository.id.contains('\\')
            || repository.id.chars().any(char::is_control)
        {
            return Err(Error::new(
                codes::ROOT_006,
                format!("repository id '{}' is not a safe file name", repository.id),
            ));
        }
        let (kind, url) = repository.location();
        let mut url = url.to_string();
        if kind == "baseurl" {
            if let Some(path) = local_repository_path(&url) {
                let path = std::fs::canonicalize(&path).map_err(|error| {
                    Error::new(
                        codes::ROOT_006,
                        format!(
                            "cannot resolve local repository {}: {error}",
                            path.display()
                        ),
                    )
                })?;
                let target = format!("/rpmx-repos/{index}");
                url = format!("file://{target}");
                mounts.push((path, target));
            }
        }
        let gpgkey = match &repository.gpgkey {
            Some(key) => match local_repository_path(key) {
                Some(path) if path.is_file() => {
                    let keys = work.join("keys");
                    std::fs::create_dir_all(&keys).map_err(|error| {
                        Error::new(
                            codes::ROOT_004,
                            format!("cannot create {}: {error}", keys.display()),
                        )
                    })?;
                    let target = keys.join(format!("{}.key", repository.id));
                    std::fs::copy(&path, &target).map_err(|error| {
                        Error::new(
                            codes::ROOT_004,
                            format!("cannot copy {}: {error}", path.display()),
                        )
                    })?;
                    Some(format!(
                        "file:///work/keys/{}",
                        target.file_name().unwrap_or_default().to_string_lossy()
                    ))
                }
                _ => Some(key.clone()),
            },
            None => None,
        };
        // Fedora dropped signed repository metadata, so only package
        // signatures are checked. Metadata pinning is an open question.
        let mut text = format!(
            "[{}]\nname={}\n{kind}={url}\nenabled=1\ngpgcheck={}\nrepo_gpgcheck=0\n",
            repository.id,
            repository.id,
            u8::from(repository.gpgcheck()),
        );
        if let Some(key) = gpgkey {
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
    Ok(mounts)
}

fn local_repository_path(value: &str) -> Option<PathBuf> {
    let path = value.strip_prefix("file://").unwrap_or(value);
    let path = PathBuf::from(path);
    if path.is_absolute() {
        Some(path)
    } else {
        None
    }
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

fn run_dnf(
    image: &str,
    work: &Path,
    local_repositories: &[(PathBuf, String)],
    args: Vec<String>,
) -> Result<()> {
    let output = podman_run(image, work, local_repositories, &args)?;
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
    fn key(&self) -> PackageKey {
        PackageKey::new(
            &self.name,
            &self.epoch.to_string(),
            &self.version,
            &self.release,
            &self.arch,
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
    local_repositories: &[(PathBuf, String)],
) -> Result<BTreeMap<PackageKey, RepositoryMatch>> {
    let mut args = dnf_base_args(profile);
    args.extend([
        "repoquery".to_string(),
        "--queryformat".to_string(),
        "%{name}|%{epoch}|%{version}|%{release}|%{arch}|%{repoid}|%{sourcerpm}\n".to_string(),
    ]);
    args.extend(specs.iter().cloned());

    let output = podman_run(image, work, local_repositories, &args)?;
    if !output.status.success() {
        return Err(Error::new(
            codes::ROOT_007,
            format!(
                "repository metadata query failed:\n{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    let mut found: BTreeMap<PackageKey, RepositoryMatch> = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields = line.split('|').collect::<Vec<_>>();
        if fields.len() != 7 {
            continue;
        }
        let key = PackageKey::new(fields[0], fields[1], fields[2], fields[3], fields[4]);
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

#[cfg(test)]
mod local_repository_tests {
    use super::write_repositories;
    use crate::profile::Repository;

    #[test]
    fn a_local_baseurl_is_mounted_and_rewritten() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let repository = tempfile::tempdir().expect("repository");
        let repositories = vec![Repository {
            id: "local".to_string(),
            baseurl: Some(format!("file://{}", repository.path().display())),
            metalink: None,
            mirrorlist: None,
            gpgkey: None,
        }];
        let mounts = write_repositories(work.path(), &repositories).expect("write");
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].1, "/rpmx-repos/0");
        let text = std::fs::read_to_string(work.path().join("repos.d/local.repo")).expect("repo");
        assert!(text.contains("baseurl=file:///rpmx-repos/0"), "{text}");
    }

    #[test]
    fn a_local_gpgkey_is_copied_into_the_work_directory() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let key = work.path().join("key.asc");
        std::fs::write(&key, b"key").expect("key");
        let repositories = vec![Repository {
            id: "local".to_string(),
            baseurl: Some("https://example.invalid".to_string()),
            metalink: None,
            mirrorlist: None,
            gpgkey: Some(format!("file://{}", key.display())),
        }];
        write_repositories(work.path(), &repositories).expect("write");
        let text = std::fs::read_to_string(work.path().join("repos.d/local.repo")).expect("repo");
        assert!(
            text.contains("gpgkey=file:///work/keys/local.key"),
            "{text}"
        );
        assert!(work.path().join("keys/local.key").is_file());
    }
}
