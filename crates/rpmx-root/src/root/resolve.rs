use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::profile::{
    has_control_characters, has_url_userinfo, Profile, Repository, RepositoryAuth,
};
use crate::root::container::{image_digest_for_arch, podman_run};
use crate::root::lock::{
    closure_digest, LockedBuilder, LockedPackage, PackageKey, RootLock, DOCUMENT, VERSION,
};
use crate::root::metadata_cache::MetadataCache;
use rpmx_base::error::{codes, Error, Result};
use rpmx_store::cas;

pub fn resolve_profile(
    profile: &Profile,
    profile_name: &str,
    work: &Path,
    cache: &Path,
) -> Result<RootLock> {
    if profile.archs.len() != 1 {
        return Err(Error::new(
            codes::ROOT_006,
            format!(
                "resolve_profile needs one target architecture; profile '{}' allows {}",
                profile_name,
                profile.archs.join(", ")
            ),
        ));
    }
    let arch = profile.arch();
    let image = profile.builder_image();
    let image_digest = image_digest_for_arch(&image, profile.arch())?;

    prepare_work(work)?;
    let _scratch = ResolverScratch {
        work: work.to_path_buf(),
    };
    let setup = write_repositories(work, &profile.repositories)?;
    let metadata_cache = MetadataCache::new(profile, &image_digest, cache);
    metadata_cache.restore(work);

    // Resolve all requests in one transaction. Downloading each request's
    // closure can keep a newer dependency beside an exact older request.
    // Store downloads the solved set without installing it in the resolver.
    run_dnf(
        &image,
        work,
        &setup.mounts,
        &setup.secrets,
        dnf_base_args(profile)
            .into_iter()
            .chain([
                "--refresh".to_string(),
                "install".to_string(),
                "--store=/work/transaction".to_string(),
            ])
            .chain(profile.packages.install.iter().cloned())
            .collect(),
    )?;

    let downloaded = read_downloaded(&work.join("transaction/packages"), cache)?;
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
    let metadata =
        query_repositories(&image, work, profile, &specs, &setup.mounts, &setup.secrets)?;

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
    // The lock is a distributable artefact. Keep the repository identity and
    // signing policy, but not credential references, which are host-specific.
    let repositories = repositories_for_lock(&profile.repositories);

    let lock = RootLock {
        document: DOCUMENT.to_string(),
        version: VERSION,
        profile: profile_name.to_string(),
        archs,
        releasever: profile.releasever.clone(),
        dist_tag: profile.dist_tag()?,
        options: profile.options.clone(),
        prefix: profile.paths.prefix.clone(),
        libdir: profile.paths.libdir_for(arch).to_string(),
        sbom: profile.sbom,
        repositories,
        packages,
        closure_digest: closure,
        root_digest: None,
        builder: Some(LockedBuilder {
            image,
            image_digest,
        }),
        seed: None,
        generation: None,
        requirement: None,
    };
    metadata_cache.publish(work);
    Ok(lock)
}

fn prepare_work(work: &Path) -> Result<()> {
    // The RPMs and the empty installroot are outputs of this run. A reused work
    // directory must not feed packages from an earlier resolution into the
    // closure. Repository definitions and credentials are regenerated too, so a
    // stale repository cannot leak into the transaction. Only remote metadata
    // from the persistent snapshot cache is copied back after this reset.
    for directory in [
        "rpms",
        "transaction",
        "root",
        "repos.d",
        "keys",
        "cache",
        "persist",
    ] {
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

#[derive(Debug)]
struct RepositorySetup {
    mounts: Vec<(PathBuf, String)>,
    secrets: Vec<String>,
    _credentials: Option<tempfile::TempDir>,
}

/// Repository definitions can contain a plaintext basic-auth password. Remove
/// them when resolution ends, even if the caller keeps the work directory.
struct ResolverScratch {
    work: PathBuf,
}

impl Drop for ResolverScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.work.join("repos.d"));
        let _ = std::fs::remove_dir_all(self.work.join("keys"));
    }
}

struct ResolvedAuth {
    ca: Option<String>,
    cert: Option<String>,
    key: Option<String>,
    username: Option<String>,
    password: Option<String>,
    proxy: Option<String>,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
    mounts: Vec<(PathBuf, String)>,
    secrets: Vec<String>,
}

fn repositories_for_lock(repositories: &[Repository]) -> Vec<Repository> {
    repositories
        .iter()
        .map(|repository| {
            let mut locked = repository.clone();
            locked.auth = None;
            locked
        })
        .collect()
}

fn write_repositories(work: &Path, repositories: &[Repository]) -> Result<RepositorySetup> {
    let mut mounts = Vec::new();
    let mut secrets = Vec::new();
    let mut credentials = None;

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
        if has_control_characters(&url) {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "repository '{}' {kind} contains a control character",
                    repository.id
                ),
            ));
        }
        if has_url_userinfo(&url) {
            return Err(Error::new(
                codes::ROOT_006,
                format!(
                    "repository '{}' embeds credentials in its {kind} URL; use the auth block instead",
                    repository.id
                ),
            ));
        }
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
        if let Some(key) = &repository.gpgkey {
            if has_control_characters(key) {
                return Err(Error::new(
                    codes::ROOT_006,
                    format!(
                        "repository '{}' gpgkey contains a control character",
                        repository.id
                    ),
                ));
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
        let auth = match &repository.auth {
            Some(auth) => {
                if credentials.is_none() {
                    credentials = Some(
                        tempfile::Builder::new()
                            .prefix("rpmx-credentials-")
                            .tempdir()
                            .map_err(|error| {
                                Error::new(
                                    codes::ROOT_004,
                                    format!("cannot create a credential directory: {error}"),
                                )
                            })?,
                    );
                }
                let resolved = resolve_auth(
                    &repository.id,
                    index,
                    auth,
                    credentials
                        .as_ref()
                        .expect("credential directory was just created")
                        .path(),
                )?;
                mounts.extend(resolved.mounts.iter().cloned());
                secrets.extend(resolved.secrets.iter().cloned());
                Some(resolved)
            }
            None => None,
        };
        // Fedora dropped signed repository metadata, so only package
        // signatures are checked by default. RHEL-style repositories may
        // require metadata signatures.
        let mut text = format!(
            "[{}]\nname={}\n{kind}={url}\nenabled=1\ngpgcheck={}\nrepo_gpgcheck={}\n",
            repository.id,
            repository.id,
            u8::from(repository.gpgcheck()),
            u8::from(repository.repo_gpgcheck),
        );
        if let Some(priority) = repository.priority {
            text.push_str(&format!("priority={priority}\n"));
        }
        if let Some(key) = gpgkey {
            text.push_str(&format!("gpgkey={key}\n"));
        }
        if !repository.exclude.is_empty() {
            text.push_str(&format!("exclude={}\n", repository.exclude.join(",")));
        }
        if let Some(auth) = &auth {
            if let Some(ca) = &auth.ca {
                text.push_str(&format!("sslcacert={ca}\n"));
            }
            if let Some(cert) = &auth.cert {
                text.push_str(&format!("sslclientcert={cert}\n"));
            }
            if let Some(key) = &auth.key {
                text.push_str(&format!("sslclientkey={key}\n"));
            }
            if let Some(username) = &auth.username {
                text.push_str(&format!("username={}\n", escape_dnf_value(username)));
            }
            if let Some(password) = &auth.password {
                text.push_str(&format!("password={}\n", escape_dnf_value(password)));
            }
            if let Some(proxy) = &auth.proxy {
                text.push_str(&format!("proxy={}\n", escape_dnf_value(proxy)));
            }
            if let Some(username) = &auth.proxy_username {
                text.push_str(&format!("proxy_username={}\n", escape_dnf_value(username)));
            }
            if let Some(password) = &auth.proxy_password {
                text.push_str(&format!("proxy_password={}\n", escape_dnf_value(password)));
            }
        }
        let path = work.join("repos.d").join(format!("{}.repo", repository.id));
        write_private_file(&path, text.as_bytes())?;
    }
    Ok(RepositorySetup {
        mounts,
        secrets,
        _credentials: credentials,
    })
}

fn resolve_auth(
    repository: &str,
    index: usize,
    auth: &RepositoryAuth,
    credentials: &Path,
) -> Result<ResolvedAuth> {
    let mut resolved = ResolvedAuth {
        ca: None,
        cert: None,
        key: None,
        username: None,
        password: None,
        proxy: None,
        proxy_username: None,
        proxy_password: None,
        mounts: Vec::new(),
        secrets: Vec::new(),
    };
    for (field, value) in [
        ("ca", auth.ca.as_deref()),
        ("cert", auth.cert.as_deref()),
        ("key", auth.key.as_deref()),
    ] {
        let Some(value) = value else {
            continue;
        };
        let source = credential_path(repository, field, value)?;
        let target = credentials.join(format!("{index}-{field}"));
        copy_private_file(&source, &target).map_err(|error| {
            Error::new(
                codes::ROOT_019,
                format!(
                    "cannot stage repository '{repository}' auth.{field} from {}: {error}",
                    source.display()
                ),
            )
        })?;
        let container = format!("/run/rpmx-auth/{index}/{field}");
        resolved.mounts.push((target, container.clone()));
        match field {
            "ca" => resolved.ca = Some(container),
            "cert" => resolved.cert = Some(container),
            "key" => resolved.key = Some(container),
            _ => unreachable!("fixed credential field list"),
        }
    }
    resolved.username = auth
        .username
        .as_deref()
        .map(|value| credential_value(repository, "username", value, true))
        .transpose()?;
    resolved.password = auth
        .password
        .as_deref()
        .map(|value| credential_value(repository, "password", value, false))
        .transpose()?;
    if let Some(password) = &resolved.password {
        push_secret(&mut resolved.secrets, password);
    }
    resolved.proxy = auth
        .proxy
        .as_deref()
        .map(|value| credential_value(repository, "proxy", value, true))
        .transpose()?;
    resolved.proxy_username = auth
        .proxy_username
        .as_deref()
        .map(|value| credential_value(repository, "proxy-username", value, true))
        .transpose()?;
    resolved.proxy_password = auth
        .proxy_password
        .as_deref()
        .map(|value| credential_value(repository, "proxy-password", value, false))
        .transpose()?;
    if let Some(password) = &resolved.proxy_password {
        push_secret(&mut resolved.secrets, password);
    }
    Ok(resolved)
}

/// Redact both the raw secret and the escaped form written to the repo file.
fn push_secret(secrets: &mut Vec<String>, secret: &str) {
    if !secrets.contains(&secret.to_string()) {
        secrets.push(secret.to_string());
    }
    let escaped = escape_dnf_value(secret);
    if escaped != secret && !secrets.contains(&escaped) {
        secrets.push(escaped);
    }
}

fn credential_path(repository: &str, field: &str, value: &str) -> Result<PathBuf> {
    let value = match value.strip_prefix("env:") {
        Some(name) => std::env::var(name).map_err(|error| {
            Error::new(
                codes::ROOT_019,
                format!(
                    "repository '{repository}' auth.{field} references environment variable {name}: {error}"
                ),
            )
        })?,
        None => value.strip_prefix("file://").unwrap_or(value).to_string(),
    };
    let path = PathBuf::from(&value);
    if !path.is_absolute() {
        return Err(Error::new(
            codes::ROOT_019,
            format!(
                "repository '{repository}' auth.{field} must be an env:NAME reference or an absolute path, found '{value}'"
            ),
        ));
    }
    let path = std::fs::canonicalize(&path).map_err(|error| {
        Error::new(
            codes::ROOT_019,
            format!(
                "cannot resolve repository '{repository}' auth.{field} {}: {error}",
                path.display()
            ),
        )
    })?;
    if !path.is_file() {
        return Err(Error::new(
            codes::ROOT_019,
            format!(
                "repository '{repository}' auth.{field} {} is not a regular file",
                path.display()
            ),
        ));
    }
    Ok(path)
}

fn credential_value(
    repository: &str,
    field: &str,
    value: &str,
    allow_literal: bool,
) -> Result<String> {
    let value = if let Some(name) = value.strip_prefix("env:") {
        std::env::var(name).map_err(|error| {
            Error::new(
                codes::ROOT_019,
                format!(
                    "repository '{repository}' auth.{field} references environment variable {name}: {error}"
                ),
            )
        })?
    } else {
        let path = value.strip_prefix("file://").unwrap_or(value);
        if Path::new(path).is_absolute() {
            read_credential_file(repository, field, Path::new(path))?
        } else if allow_literal {
            value.to_string()
        } else {
            return Err(Error::new(
                codes::ROOT_019,
                format!(
                    "repository '{repository}' auth.{field} must be an env:NAME reference or a file path"
                ),
            ));
        }
    };
    if value.is_empty() {
        return Err(Error::new(
            codes::ROOT_019,
            format!("repository '{repository}' auth.{field} is empty"),
        ));
    }
    if has_control_characters(&value) {
        return Err(Error::new(
            codes::ROOT_019,
            format!("repository '{repository}' auth.{field} contains a control character"),
        ));
    }
    Ok(value)
}

fn read_credential_file(repository: &str, field: &str, path: &Path) -> Result<String> {
    const MAX_CREDENTIAL_BYTES: u64 = 64 * 1024;
    let path = std::fs::canonicalize(path).map_err(|error| {
        Error::new(
            codes::ROOT_019,
            format!(
                "cannot resolve repository '{repository}' auth.{field} {}: {error}",
                path.display()
            ),
        )
    })?;
    let metadata = std::fs::metadata(&path).map_err(|error| {
        Error::new(
            codes::ROOT_019,
            format!(
                "cannot read repository '{repository}' auth.{field} {}: {error}",
                path.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(Error::new(
            codes::ROOT_019,
            format!(
                "repository '{repository}' auth.{field} {} is not a regular file",
                path.display()
            ),
        ));
    }
    if metadata.len() > MAX_CREDENTIAL_BYTES {
        return Err(Error::new(
            codes::ROOT_019,
            format!(
                "repository '{repository}' auth.{field} {} exceeds the {MAX_CREDENTIAL_BYTES}-byte limit",
                path.display()
            ),
        ));
    }
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Error::new(
            codes::ROOT_019,
            format!(
                "cannot read repository '{repository}' auth.{field} {}: {error}",
                path.display()
            ),
        )
    })?;
    let text = text.trim_end_matches(['\r', '\n']).to_string();
    if text.is_empty() {
        return Err(Error::new(
            codes::ROOT_019,
            format!(
                "repository '{repository}' auth.{field} {} is empty",
                path.display()
            ),
        ));
    }
    Ok(text)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let result = {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .and_then(|mut file| {
                    file.write_all(bytes)?;
                    file.set_permissions(std::fs::Permissions::from_mode(0o600))
                })
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, bytes)
        }
    };
    result.map_err(|error| {
        Error::new(
            codes::ROOT_004,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}

fn copy_private_file(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        let mut input = std::fs::File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(target)?;
        std::io::copy(&mut input, &mut output)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(source, target).map(|_| ())
    }
}

fn escape_dnf_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '$') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn redact(text: &str, secrets: &[String]) -> String {
    let mut text = text.to_string();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        text = text.replace(secret, "***");
    }
    text
}

fn looks_like_auth_failure(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "401 unauthorized",
        "403 forbidden",
        "status code: 401",
        "status code: 403",
        "unauthorized",
        "forbidden",
        "authentication",
        "certificate",
        "entitlement",
        "login expired",
    ]
    .iter()
    .any(|needle| text.contains(needle))
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
        "--assumeyes".to_string(),
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
    mounts: &[(PathBuf, String)],
    secrets: &[String],
    args: Vec<String>,
) -> Result<()> {
    let output = podman_run(image, work, mounts, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(repository_failure(
            "dnf resolution failed",
            &stderr,
            secrets,
        ));
    }
    Ok(())
}

fn repository_failure(context: &str, stderr: &str, secrets: &[String]) -> Error {
    let code = if looks_like_auth_failure(stderr) {
        codes::ROOT_019
    } else {
        codes::ROOT_007
    };
    let tail = stderr
        .lines()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Error::new(code, format!("{context}:\n{}", redact(&tail, secrets)))
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
    mounts: &[(PathBuf, String)],
    secrets: &[String],
) -> Result<BTreeMap<PackageKey, RepositoryMatch>> {
    let mut args = dnf_base_args(profile);
    args.extend([
        "repoquery".to_string(),
        "--queryformat".to_string(),
        "%{name}|%{epoch}|%{version}|%{release}|%{arch}|%{repoid}|%{sourcerpm}\n".to_string(),
    ]);
    args.extend(specs.iter().cloned());

    let output = podman_run(image, work, mounts, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(repository_failure(
            "repository metadata query failed",
            &stderr,
            secrets,
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
    use super::{
        prepare_work, redact, repositories_for_lock, repository_failure, write_repositories,
        ResolverScratch,
    };
    use crate::profile::{Repository, RepositoryAuth};

    fn repository(id: &str) -> Repository {
        Repository {
            id: id.to_string(),
            baseurl: Some("https://example.invalid/fedora".to_string()),
            metalink: None,
            mirrorlist: None,
            gpgkey: None,
            repo_gpgcheck: false,
            exclude: Vec::new(),
            priority: None,
            auth: None,
        }
    }

    #[test]
    fn a_local_baseurl_is_mounted_and_rewritten() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let repository_dir = tempfile::tempdir().expect("repository");
        let mut repository_entry = repository("local");
        repository_entry.baseurl = Some(format!("file://{}", repository_dir.path().display()));
        let setup = write_repositories(work.path(), &[repository_entry]).expect("write");
        assert_eq!(setup.mounts.len(), 1);
        assert_eq!(setup.mounts[0].1, "/rpmx-repos/0");
        let text = std::fs::read_to_string(work.path().join("repos.d/local.repo")).expect("repo");
        assert!(text.contains("baseurl=file:///rpmx-repos/0"), "{text}");
    }

    #[test]
    fn a_reused_work_directory_drops_the_previous_transaction() {
        let directory = tempfile::tempdir().unwrap();
        let packages = directory.path().join("transaction/packages");
        std::fs::create_dir_all(&packages).unwrap();
        std::fs::write(packages.join("stale.rpm"), "stale payload").unwrap();
        std::fs::create_dir_all(directory.path().join("cache/local/repodata")).unwrap();
        std::fs::write(
            directory.path().join("cache/local/repodata/primary.xml"),
            "stale metadata",
        )
        .unwrap();
        std::fs::create_dir_all(directory.path().join("persist")).unwrap();
        std::fs::write(directory.path().join("persist/state"), "stale state").unwrap();
        prepare_work(directory.path()).unwrap();
        assert!(!directory.path().join("transaction").exists());
        assert!(directory.path().join("cache").is_dir());
        assert!(!directory.path().join("cache/local").exists());
        assert!(!directory.path().join("persist/state").exists());
    }

    #[test]
    fn a_local_gpgkey_is_copied_into_the_work_directory() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let key = work.path().join("key.asc");
        std::fs::write(&key, b"key").expect("key");
        let mut repository_entry = repository("local");
        repository_entry.gpgkey = Some(format!("file://{}", key.display()));
        write_repositories(work.path(), &[repository_entry]).expect("write");
        let text = std::fs::read_to_string(work.path().join("repos.d/local.repo")).expect("repo");
        assert!(
            text.contains("gpgkey=file:///work/keys/local.key"),
            "{text}"
        );
        assert!(work.path().join("keys/local.key").is_file());
    }

    #[test]
    fn tls_client_credentials_are_staged_and_referenced() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let credentials = tempfile::tempdir().expect("credentials");
        let ca = credentials.path().join("ca.pem");
        let cert = credentials.path().join("client.pem");
        let key = credentials.path().join("client.key");
        std::fs::write(&ca, b"ca").expect("ca");
        std::fs::write(&cert, b"cert").expect("cert");
        std::fs::write(&key, b"key").expect("key");

        let mut repository_entry = repository("rhel");
        repository_entry.auth = Some(RepositoryAuth {
            ca: Some(ca.display().to_string()),
            cert: Some(cert.display().to_string()),
            key: Some(key.display().to_string()),
            ..RepositoryAuth::default()
        });
        let setup = write_repositories(work.path(), &[repository_entry]).expect("write");
        let text = std::fs::read_to_string(work.path().join("repos.d/rhel.repo")).expect("repo");
        assert_eq!(setup.mounts.len(), 3);
        assert!(text.contains("sslcacert=/run/rpmx-auth/0/ca"), "{text}");
        assert!(
            text.contains("sslclientcert=/run/rpmx-auth/0/cert"),
            "{text}"
        );
        assert!(text.contains("sslclientkey=/run/rpmx-auth/0/key"), "{text}");
        for (source, target) in &setup.mounts {
            assert!(source.is_file(), "{}", source.display());
            assert!(target.starts_with("/run/rpmx-auth/0/"), "{target}");
            let expected = match target.rsplit('/').next().unwrap_or_default() {
                "ca" => b"ca".as_slice(),
                "cert" => b"cert".as_slice(),
                "key" => b"key".as_slice(),
                other => panic!("unexpected credential target {other}"),
            };
            assert_eq!(std::fs::read(source).unwrap(), expected, "{target}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for (source, _) in &setup.mounts {
                let mode = std::fs::metadata(source).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600, "{}", source.display());
            }
        }
    }

    #[test]
    fn staged_credentials_disappear_with_the_setup() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let cert = work.path().join("client.pem");
        let key = work.path().join("client.key");
        std::fs::write(&cert, b"cert").expect("cert");
        std::fs::write(&key, b"key").expect("key");
        let mut repository_entry = repository("rhel");
        repository_entry.auth = Some(RepositoryAuth {
            cert: Some(cert.display().to_string()),
            key: Some(key.display().to_string()),
            ..RepositoryAuth::default()
        });
        let setup = write_repositories(work.path(), &[repository_entry]).expect("write");
        let staged = setup.mounts[0].0.clone();
        assert!(staged.is_file());
        assert!(!staged.starts_with(work.path()));
        drop(setup);
        assert!(!staged.exists());
    }

    #[test]
    fn basic_auth_reads_the_password_from_the_environment() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let variable = format!("RPMX_TEST_PASSWORD_{}", std::process::id());
        std::env::set_var(&variable, "s3cret");
        let mut repository_entry = repository("satellite");
        repository_entry.auth = Some(RepositoryAuth {
            username: Some("alice".to_string()),
            password: Some(format!("env:{variable}")),
            ..RepositoryAuth::default()
        });
        let setup = write_repositories(work.path(), &[repository_entry]).expect("write");
        std::env::remove_var(&variable);
        let text =
            std::fs::read_to_string(work.path().join("repos.d/satellite.repo")).expect("repo");
        assert!(text.contains("username=alice"), "{text}");
        assert!(text.contains("password=s3cret"), "{text}");
        assert_eq!(setup.secrets, vec!["s3cret".to_string()]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(work.path().join("repos.d/satellite.repo"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_missing_credential_environment_variable_is_an_auth_error() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let mut repository_entry = repository("satellite");
        repository_entry.auth = Some(RepositoryAuth {
            username: Some("alice".to_string()),
            password: Some("env:RPMX_TEST_MISSING_PASSWORD".to_string()),
            ..RepositoryAuth::default()
        });
        let error = write_repositories(work.path(), &[repository_entry]).expect_err("auth");
        assert_eq!(error.code, "RPMX-ROOT-019");
    }

    #[test]
    fn a_password_is_escaped_for_dnf_variable_expansion() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let variable = format!("RPMX_TEST_ESCAPE_PASSWORD_{}", std::process::id());
        std::env::set_var(&variable, "s3cret$releasever\\x");
        let mut repository_entry = repository("satellite");
        repository_entry.auth = Some(RepositoryAuth {
            username: Some("alice".to_string()),
            password: Some(format!("env:{variable}")),
            ..RepositoryAuth::default()
        });
        let setup = write_repositories(work.path(), &[repository_entry]).expect("write");
        std::env::remove_var(&variable);
        let text =
            std::fs::read_to_string(work.path().join("repos.d/satellite.repo")).expect("repo");
        assert!(text.contains("password=s3cret\\$releasever\\\\x"), "{text}");
        assert!(
            setup
                .secrets
                .contains(&"s3cret\\$releasever\\\\x".to_string()),
            "{:?}",
            setup.secrets
        );
    }

    #[test]
    fn an_empty_environment_password_is_rejected() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let variable = format!("RPMX_TEST_EMPTY_PASSWORD_{}", std::process::id());
        std::env::set_var(&variable, "");
        let mut repository_entry = repository("satellite");
        repository_entry.auth = Some(RepositoryAuth {
            username: Some("alice".to_string()),
            password: Some(format!("env:{variable}")),
            ..RepositoryAuth::default()
        });
        let error = write_repositories(work.path(), &[repository_entry]).expect_err("empty");
        std::env::remove_var(&variable);
        assert_eq!(error.code, "RPMX-ROOT-019");
        assert!(error.message.contains("is empty"), "{}", error.message);
    }

    #[test]
    fn a_credential_with_a_newline_is_rejected() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let variable = format!("RPMX_TEST_NEWLINE_PASSWORD_{}", std::process::id());
        std::env::set_var(&variable, "valid\nsslverify=0");
        let mut repository_entry = repository("satellite");
        repository_entry.auth = Some(RepositoryAuth {
            username: Some("alice".to_string()),
            password: Some(format!("env:{variable}")),
            ..RepositoryAuth::default()
        });
        let error = write_repositories(work.path(), &[repository_entry]).expect_err("newline");
        std::env::remove_var(&variable);
        assert_eq!(error.code, "RPMX-ROOT-019");
        assert!(
            error.message.contains("control character"),
            "{}",
            error.message
        );
    }

    #[test]
    fn an_oversized_password_file_is_rejected() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let password = work.path().join("password");
        std::fs::write(&password, vec![b'x'; 64 * 1024 + 1]).expect("password");
        let mut repository_entry = repository("satellite");
        repository_entry.auth = Some(RepositoryAuth {
            username: Some("alice".to_string()),
            password: Some(password.display().to_string()),
            ..RepositoryAuth::default()
        });
        let error = write_repositories(work.path(), &[repository_entry]).expect_err("oversized");
        assert_eq!(error.code, "RPMX-ROOT-019");
        assert!(error.message.contains("exceeds"), "{}", error.message);
    }

    #[test]
    fn a_password_file_is_read_from_disk() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let password = work.path().join("password");
        std::fs::write(&password, b"from-file\n").expect("password");
        let mut repository_entry = repository("satellite");
        repository_entry.auth = Some(RepositoryAuth {
            username: Some("alice".to_string()),
            password: Some(password.display().to_string()),
            ..RepositoryAuth::default()
        });
        let setup = write_repositories(work.path(), &[repository_entry]).expect("write");
        let text =
            std::fs::read_to_string(work.path().join("repos.d/satellite.repo")).expect("repo");
        assert!(text.contains("password=from-file"), "{text}");
        assert_eq!(setup.secrets, vec!["from-file".to_string()]);
    }

    #[test]
    fn repo_gpgcheck_is_written_when_requested() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let mut repository_entry = repository("rhel");
        repository_entry.repo_gpgcheck = true;
        write_repositories(work.path(), &[repository_entry]).expect("write");
        let text = std::fs::read_to_string(work.path().join("repos.d/rhel.repo")).expect("repo");
        assert!(text.contains("repo_gpgcheck=1"), "{text}");
    }

    #[test]
    fn repo_gpgcheck_is_disabled_by_default() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        write_repositories(work.path(), &[repository("rhel")]).expect("write");
        let text = std::fs::read_to_string(work.path().join("repos.d/rhel.repo")).expect("repo");
        assert!(text.contains("repo_gpgcheck=0"), "{text}");
    }

    #[test]
    fn repository_excludes_are_written_to_the_repo_file() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let mut repository_entry = repository("rhel");
        repository_entry.exclude = vec!["glibc".to_string(), "gcc".to_string()];
        write_repositories(work.path(), &[repository_entry]).expect("write");
        let text = std::fs::read_to_string(work.path().join("repos.d/rhel.repo")).expect("repo");
        assert!(text.contains("exclude=glibc,gcc"), "{text}");
    }

    #[test]
    fn repository_excludes_are_absent_by_default() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        write_repositories(work.path(), &[repository("rhel")]).expect("write");
        let text = std::fs::read_to_string(work.path().join("repos.d/rhel.repo")).expect("repo");
        assert!(!text.contains("exclude="), "{text}");
    }

    #[test]
    fn resolver_scratch_removes_repository_definitions() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        std::fs::create_dir_all(work.path().join("keys")).expect("keys");
        {
            let _scratch = ResolverScratch {
                work: work.path().to_path_buf(),
            };
        }
        assert!(!work.path().join("repos.d").exists());
        assert!(!work.path().join("keys").exists());
    }

    #[test]
    fn proxy_credentials_are_written_and_registered_for_redaction() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let variable = format!("RPMX_TEST_PROXY_PASSWORD_{}", std::process::id());
        std::env::set_var(&variable, "proxy-secret");
        let mut repository_entry = repository("satellite");
        repository_entry.auth = Some(RepositoryAuth {
            proxy: Some("http://proxy.invalid:3128".to_string()),
            proxy_username: Some("proxy-user".to_string()),
            proxy_password: Some(format!("env:{variable}")),
            ..RepositoryAuth::default()
        });
        let setup = write_repositories(work.path(), &[repository_entry]).expect("write");
        std::env::remove_var(&variable);
        let text =
            std::fs::read_to_string(work.path().join("repos.d/satellite.repo")).expect("repo");
        assert!(text.contains("proxy=http://proxy.invalid:3128"), "{text}");
        assert!(text.contains("proxy_username=proxy-user"), "{text}");
        assert!(text.contains("proxy_password=proxy-secret"), "{text}");
        assert_eq!(setup.secrets, vec!["proxy-secret".to_string()]);
    }

    #[test]
    fn lock_repositories_drop_credentials_but_keep_signing_policy() {
        let mut repository_entry = repository("rhel");
        repository_entry.repo_gpgcheck = true;
        repository_entry.gpgkey = Some("https://example.invalid/key".to_string());
        repository_entry.auth = Some(RepositoryAuth {
            username: Some("alice".to_string()),
            password: Some("env:RPMX_TEST_LOCK_PASSWORD".to_string()),
            ..RepositoryAuth::default()
        });
        let locked = repositories_for_lock(&[repository_entry]);
        assert!(locked[0].auth.is_none());
        assert!(locked[0].repo_gpgcheck);
        assert_eq!(
            locked[0].gpgkey.as_deref(),
            Some("https://example.invalid/key")
        );
    }

    #[test]
    fn dnf_auth_failures_are_classified_and_redacted() {
        let error = repository_failure(
            "dnf resolution failed",
            "HTTP 401 Unauthorized for password s3cret",
            &["s3cret".to_string()],
        );
        assert_eq!(error.code, "RPMX-ROOT-019");
        assert!(!error.message.contains("s3cret"), "{}", error.message);
        assert!(error.message.contains("***"), "{}", error.message);

        let error =
            repository_failure("dnf resolution failed", "nothing provides libfoo.so.1", &[]);
        assert_eq!(error.code, "RPMX-ROOT-007");
    }

    #[test]
    fn credentials_in_a_repository_url_are_rejected() {
        let work = tempfile::tempdir().expect("work");
        std::fs::create_dir_all(work.path().join("repos.d")).expect("repos.d");
        let mut repository_entry = repository("rhel");
        repository_entry.baseurl = Some("https://user:secret@example.invalid/fedora".to_string());
        let error = write_repositories(work.path(), &[repository_entry]).expect_err("userinfo");
        assert_eq!(error.code, "RPMX-ROOT-006");
        assert!(error.message.contains("use the auth block"));
    }

    #[test]
    fn secrets_are_removed_from_error_text() {
        let text = "failed with password s3cret and s3cret again";
        assert_eq!(
            redact(text, &["s3cret".to_string()]),
            "failed with password *** and *** again"
        );
    }
}
