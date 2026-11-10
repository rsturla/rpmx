//! Installed-stage test roots.
//!
//! Test roots share `root::resolve_profile` and `root::assemble` with build
//! roots. They deliberately do not share the build root's profile: a build
//! root holds the compiler and build-time dependencies, while a test root must
//! be a disposable runtime image seeded with the test environment and the
//! selected RPMs' runtime requirements. Tests with different outputs or
//! explicit test packages therefore get separate roots.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::profile::{Packages, Profile, Repository};
use crate::root::{self, RootLock};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};
use rpmx_payload::metadata::package_metadata;
use rpmx_runner::runner::emulation_available;

/// Repository id of the packages under test inside the test root's profile.
const UNDER_TEST: &str = "test-under-test";

pub struct TestRootOptions<'a> {
    pub lock: &'a RootLock,
    /// Assemble here instead of a scratch directory, so the root can be
    /// shipped to the machine that runs the tests.
    pub destination: Option<&'a Path>,
    pub pinned: Option<&'a Path>,
    pub tools: &'a [String],
    pub rpms: &'a [PathBuf],
    /// Directory holding exactly `rpms` and their `repodata`, so the resolver
    /// can install the packages under test as if they had been published.
    pub repo: &'a Path,
    pub image: Option<&'a str>,
    pub cache: &'a Path,
    pub work: &'a Path,
}

pub struct PreparedTestRoot {
    pub path: PathBuf,
    pub lock: RootLock,
}

/// The packages under test, in selection order, as exact `name-evr.arch` specs.
///
/// They are installed from the local repository so the resolver can satisfy
/// their requirements the way it would for a published package: by name, by
/// soname, and by file. Asking for the exact build keeps the resolver on the
/// package under test instead of a repository copy with a higher release, and
/// the packages themselves are removed from the resolved lock afterwards
/// because they still come from the build output.
fn under_test_packages(rpms: &[PathBuf]) -> Result<Vec<(String, String)>> {
    let mut packages: Vec<(String, String)> = Vec::new();
    for path in rpms {
        let metadata = package_metadata(path)?;
        let spec = format!("{}-{}.{}", metadata.name, metadata.evr(), metadata.arch);
        if !packages.iter().any(|(_, known)| known == &spec) {
            packages.push((metadata.name, spec));
        }
    }
    Ok(packages)
}

fn add_under_test(install: &mut Vec<String>, packages: Vec<(String, String)>) {
    for (name, spec) in packages {
        // An exact request already satisfies the generic request for this
        // package. Keeping both makes DNF demand the newest published version.
        install.retain(|request| request != &name);
        if !install.contains(&spec) {
            install.push(spec);
        }
    }
}

/// The repository that holds the packages under test, mounted for the resolver.
fn under_test_repository(repo: &Path) -> Repository {
    Repository {
        id: UNDER_TEST.to_string(),
        baseurl: Some(format!("file://{}", repo.display())),
        metalink: None,
        mirrorlist: None,
        gpgkey: None,
        repo_gpgcheck: false,
        exclude: Vec::new(),
        // Lowest priority, so a published copy of a package satisfies another
        // package's requirement instead of the build under test. The exact
        // `name-evr.arch` specs still select the build itself.
        priority: Some(999),
        auth: None,
    }
}

/// Drop the packages that came from the under-test repository.
///
/// The lock describes the disposable root: the packages that came from a
/// published repository. The packages under test are installed from the build
/// output afterwards, so they neither belong in the lock nor in the repository
/// list that a pinned lock replays. A published package of the same name stays,
/// because another locked package may require its exact EVR.
fn without_under_test(lock: &mut RootLock) {
    lock.repositories.retain(|repo| repo.id != UNDER_TEST);
    for packages in lock.packages.values_mut() {
        packages.retain(|package| package.repo_id != UNDER_TEST);
    }
    lock.closure_digest = root::closure_digest(
        &lock.releasever,
        &lock.archs,
        &lock.repositories,
        &lock.packages,
    );
}

/// Build and install a disposable root for installed-stage tests.
pub fn prepare(options: TestRootOptions<'_>) -> Result<PreparedTestRoot> {
    let lock = options.lock;
    let target = Arch::parse(
        lock.archs
            .first()
            .ok_or_else(|| Error::new(codes::BUILD_007, "the test root has no architecture"))?,
    )?;
    let host = Arch::current()?;
    if host != target && !emulation_available(target) {
        return Err(Error::new(
            codes::RUNNER_002,
            format!(
                "cannot prepare a {target} test root on a {host} host; \
                 no qemu-user binfmt handler is available"
            ),
        ));
    }
    let image = options
        .image
        .or_else(|| lock.builder.as_ref().map(|builder| builder.image.as_str()))
        .ok_or_else(|| {
            Error::new(
                codes::BUILD_007,
                "installed-stage tests need a builder image in the root lock",
            )
        })?;

    let mut install = vec!["bash".to_string(), "coreutils".to_string()];
    for tool in options.tools {
        let name = rpmx_runner::build_env::dependency_name(tool);
        if !name.is_empty() && !install.iter().any(|package| package == name) {
            install.push(name.to_string());
        }
    }
    // The packages under test are not published, so a local repository holds
    // them for the resolver. Resolving them like any other package keeps the
    // test root's dependency handling identical to an install: sonames, file
    // requirements and virtual provides all resolve from the same transaction,
    // and a packager never has to spell out the libraries a package links.
    let mut repositories = lock.repositories.clone();
    if options.pinned.is_none() {
        add_under_test(&mut install, under_test_packages(options.rpms)?);
        repositories.insert(0, under_test_repository(options.repo));
    }

    let profile = Profile {
        document: crate::profile::DOCUMENT.to_string(),
        version: crate::profile::VERSION,
        name: Some("test-root".to_string()),
        releasever: lock.releasever.clone(),
        dist_tag: lock.dist_tag.clone(),
        archs: lock.archs.clone(),
        builder: crate::profile::Builder {
            image: Some(image.to_string()),
        },
        repositories,
        packages: Packages {
            install: install.clone(),
            reinstall: Vec::new(),
        },
        cross: None,
        modules: Default::default(),
        options: lock.options.clone(),
        paths: crate::profile::Paths {
            prefix: lock.prefix.clone(),
            libdir: lock.libdir.clone(),
        },
        sbom: false,
    };

    let resolve_work = options.work.join("test-resolve");
    let assemble_work = options.work.join("test-assemble");
    let root_path = match options.destination {
        Some(destination) => destination.to_path_buf(),
        None => options.work.join("test-root"),
    };
    let test_lock = match options.pinned {
        Some(path) => {
            let pinned = root::load_lock(path)?;
            check_lock(path, &pinned, &install, &lock.archs)?;
            pinned
        }
        None => {
            let mut resolved =
                root::resolve_profile(&profile, "test-root", &resolve_work, options.cache)?;
            without_under_test(&mut resolved);
            resolved
        }
    };
    if options.rpms.is_empty() {
        return Err(Error::new(
            codes::BUILD_007,
            "no binary RPMs were selected for the test root",
        ));
    }
    // podman treats a relative -v source as a named volume.
    let rpms = options
        .rpms
        .iter()
        .map(|path| {
            std::fs::canonicalize(path).map_err(|error| {
                Error::new(
                    codes::BUILD_007,
                    format!("cannot resolve {}: {error}", path.display()),
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // The packages under test join the transaction that installs the lock, so
    // a locked package can be satisfied by one of them and a replacement of a
    // default implementation takes the same retry as an upgrade would.
    root::assemble(
        &test_lock,
        root::AssembleOptions {
            cache: options.cache,
            work: &assemble_work,
            root: &root_path,
            extra_rpms: &rpms,
        },
    )?;

    if let Some(destination) = options.destination {
        // The root's own top directory is read-only once the filesystem
        // package is installed, so the lock lives beside the root instead of
        // inside it. A shipped directory plus this file is self-contained.
        let lock_path = destination.with_extension("lock.yaml");
        crate::root::write_lock(&lock_path, &test_lock)?;
    }

    Ok(PreparedTestRoot {
        path: root_path,
        lock: test_lock,
    })
}

/// A pinned test root must contain the packages the recipe asks tests to install,
/// otherwise the lock and the recipe have drifted apart.
/// Validate a pinned test-root lock.
///
/// The lock is the authority: it was produced by resolving the same test
/// dependencies, and resolution may have substituted a virtual provide
/// (`zlib` resolves to `zlib-ng-compat`), which the lock records under the
/// provider's name. Only the fixed base environment is checked by name, so a
/// pinned root stays usable without guessing provider names.
fn check_lock(path: &Path, lock: &RootLock, _required: &[String], archs: &[String]) -> Result<()> {
    let arch = archs
        .first()
        .ok_or_else(|| Error::new(codes::BUILD_007, "the test root lock has no architectures"))?;
    let installed = lock.packages.get(arch).ok_or_else(|| {
        Error::new(
            codes::BUILD_007,
            format!("the test root lock has no packages for {arch}"),
        )
    })?;
    let names: BTreeSet<&str> = installed
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let missing: Vec<&str> = ["bash", "coreutils"]
        .into_iter()
        .filter(|name| !names.contains(name))
        .collect();
    if !missing.is_empty() {
        return Err(Error::new(
            codes::BUILD_007,
            format!(
                "test root lock {} is missing test dependencies: {}",
                path.display(),
                missing.join(", ")
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn exact_test_packages_replace_only_matching_generic_requests() {
        let mut install = vec!["bash".into(), "coreutils".into(), "python3".into()];
        super::add_under_test(
            &mut install,
            vec![("coreutils".into(), "coreutils-1-1.x86_64".into())],
        );
        assert_eq!(install, ["bash", "python3", "coreutils-1-1.x86_64"]);
    }

    #[test]
    fn the_packages_under_test_leave_the_lock() {
        use crate::profile::Repository;
        use crate::root::LockedPackage;

        let package = |name: &str, repo_id: &str| LockedPackage {
            name: name.to_string(),
            repo_id: repo_id.to_string(),
            evr: "1-1".to_string(),
            checksum: "sha256:00".to_string(),
            size: 1,
            srpm: None,
            module: None,
            location: None,
        };
        let mut lock = super::RootLock {
            document: "rpmx-root-lock".to_string(),
            version: 1,
            profile: "bootstrap".to_string(),
            archs: vec!["x86_64".to_string()],
            releasever: "44".to_string(),
            dist_tag: None,
            options: Default::default(),
            prefix: "/usr".to_string(),
            libdir: "lib64".to_string(),
            sbom: false,
            repositories: vec![
                Repository {
                    id: super::UNDER_TEST.to_string(),
                    baseurl: Some("file:///tmp/rpms".to_string()),
                    metalink: None,
                    mirrorlist: None,
                    gpgkey: None,
                    repo_gpgcheck: false,
                    exclude: Vec::new(),
                    priority: None,
                    auth: None,
                },
                Repository {
                    id: "fedora".to_string(),
                    metalink: Some("https://mirrors.fedoraproject.org".to_string()),
                    baseurl: None,
                    mirrorlist: None,
                    gpgkey: None,
                    repo_gpgcheck: false,
                    exclude: Vec::new(),
                    priority: None,
                    auth: None,
                },
            ],
            packages: [(
                "x86_64".to_string(),
                vec![
                    package("gettext", super::UNDER_TEST),
                    package("libxml2", "fedora"),
                ],
            )]
            .into_iter()
            .collect(),
            closure_digest: "sha256:00".to_string(),
            root_digest: None,
            builder: None,
            generation: None,
            requirement: None,
            seed: None,
        };

        let recorded = lock.closure_digest.clone();
        super::without_under_test(&mut lock);
        assert_ne!(
            lock.closure_digest, recorded,
            "the closure digest follows the stripped closure"
        );
        assert_eq!(
            lock.closure_digest,
            super::root::closure_digest(
                &lock.releasever,
                &lock.archs,
                &lock.repositories,
                &lock.packages
            )
        );
        assert_eq!(lock.repositories.len(), 1);
        assert_eq!(lock.repositories[0].id, "fedora");
        assert_eq!(
            lock.packages["x86_64"]
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>(),
            vec!["libxml2"]
        );
    }
}
