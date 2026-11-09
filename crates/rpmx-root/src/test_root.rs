//! Installed-stage test roots.
//!
//! Test roots share `root::resolve_profile` and `root::assemble` with build
//! roots. They deliberately do not share the build root's profile: a build
//! root holds the compiler and build-time dependencies, while a test root must
//! be a disposable runtime image seeded with the test environment and the
//! selected RPMs' runtime requirements. Tests with different outputs or
//! explicit test packages therefore get separate roots.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::profile::{Packages, Profile};
use crate::root::{self, RootLock};
use rpmx_base::arch::Arch;
use rpmx_base::error::{codes, Error, Result};
use rpmx_payload::metadata::package_metadata;
use rpmx_runner::runner::emulation_available;

pub struct TestRootOptions<'a> {
    pub lock: &'a RootLock,
    /// Assemble here instead of a scratch directory, so the root can be
    /// shipped to the machine that runs the tests.
    pub destination: Option<&'a Path>,
    pub pinned: Option<&'a Path>,
    pub tools: &'a [String],
    pub rpms: &'a [PathBuf],
    pub image: Option<&'a str>,
    pub cache: &'a Path,
    pub work: &'a Path,
}

pub struct PreparedTestRoot {
    pub path: PathBuf,
    pub lock: RootLock,
}

/// Runtime requirements of the built RPMs that must come from the test root
/// repositories. Capabilities provided by another selected RPM are omitted,
/// because those packages are installed from the build output, not resolved
/// from a repository.
fn external_requirements(rpms: &[PathBuf]) -> Result<Vec<String>> {
    let mut provided: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut required = Vec::new();
    for path in rpms {
        let metadata = package_metadata(path)?;
        for dependency in &metadata.provides {
            provided
                .entry(dependency.name.clone())
                .or_default()
                .push(dependency.version.clone());
        }
        provided
            .entry(metadata.name.clone())
            .or_default()
            .push(metadata.evr());
        for file in &metadata.files {
            provided.entry(file.clone()).or_default();
        }
        required.extend(metadata.requires);
    }
    Ok(missing_requirements(required, &provided))
}

fn missing_requirements(
    mut required: Vec<rpm::Dependency>,
    provided: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    required.sort_by(|left, right| {
        (&left.name, left.flags.bits(), &left.version).cmp(&(
            &right.name,
            right.flags.bits(),
            &right.version,
        ))
    });
    required.dedup_by(|left, right| {
        left.name == right.name && left.flags == right.flags && left.version == right.version
    });
    required
        .into_iter()
        .filter(|requirement| {
            is_package_requirement(&requirement.name)
                && !provided
                    .get(&requirement.name)
                    .is_some_and(|versions| satisfies(requirement, versions))
        })
        .map(requirement_spec)
        .collect()
}

/// Whether a requirement names a package rather than a file or a library
/// soname. Resolving a soname and a package name independently can select two
/// different providers for the same library (for example Fedora's `pcre2` for
/// the name and Eternal's `pcre2-libs` for the soname), and the transaction
/// then fails on the file conflict. The seeded root already supplies sonames;
/// only the package-level requirements need an explicit install.
fn is_package_requirement(name: &str) -> bool {
    !name.starts_with("rpmlib(")
        && !name.starts_with("config(")
        && !name.starts_with("rtld(")
        && !name.starts_with('/')
        && !name.contains(".so")
}

fn satisfies(requirement: &rpm::Dependency, versions: &[String]) -> bool {
    let comparator = requirement.flags.comparator_str();
    if comparator.is_empty() {
        return true;
    }
    versions.iter().any(|provided| {
        if provided.is_empty() {
            return false;
        }
        let ordering = rpmx_base::version::compare_evr(provided, &requirement.version);
        match comparator {
            "=" => ordering == std::cmp::Ordering::Equal,
            "<" => ordering == std::cmp::Ordering::Less,
            "<=" => ordering != std::cmp::Ordering::Greater,
            ">" => ordering == std::cmp::Ordering::Greater,
            ">=" => ordering != std::cmp::Ordering::Less,
            _ => false,
        }
    })
}

fn requirement_spec(requirement: rpm::Dependency) -> String {
    let comparator = requirement.flags.comparator_str();
    if comparator.is_empty() || requirement.version.is_empty() {
        requirement.name
    } else {
        format!(
            "{} {} {}",
            requirement.name, comparator, requirement.version
        )
    }
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
    // The built RPMs are not in the repository closure yet, so resolve their
    // external runtime requirements explicitly. Requirements provided by the
    // selected RPMs themselves stay out of the repository lookup: the built
    // packages are installed after the root has been assembled.
    if options.pinned.is_none() {
        for requirement in external_requirements(options.rpms)? {
            if !install.iter().any(|package| package == &requirement) {
                install.push(requirement);
            }
        }
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
        repositories: lock.repositories.clone(),
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
        None => root::resolve_profile(&profile, "test-root", &resolve_work, options.cache)?,
    };
    root::assemble(
        &test_lock,
        root::AssembleOptions {
            cache: options.cache,
            work: &assemble_work,
            root: &root_path,
        },
    )?;

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
    let install = |nodeps: bool| {
        crate::root::container::run_offline_root(crate::root::container::OfflineRootCommand {
            root: &root_path,
            image,
            target,
            nodeps,
            mode: crate::root::container::OfflineRootMode::Install { rpms: &rpms },
        })
    };
    let mut output = install(false)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if is_replacement_conflict(&stderr) {
            rpmx_base::output::warning(
                "test-root",
                "the package under test conflicts with a package the test root \
                 already has; replacing it for the test run",
            );
            output = install(true)?;
        }
    }
    if !output.status.success() {
        return Err(Error::new(
            codes::BUILD_007,
            format!(
                "cannot install the built RPMs into the test root: {}; \
                 add missing runtime packages to the test's \
                 environment.contents.packages or pass a pinned \
                 --test-root-lock",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

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
/// Whether an install failure is a replacement conflict rather than a missing
/// requirement.
///
/// A package under test may deliberately replace what the test root already
/// has, for example a package that provides the same library as the
/// distribution's default implementation. rpm reports that as a conflict, and
/// installing with `--nodeps` resolves it. A genuine missing requirement must
/// stay an error, so a failure that mentions one never takes the retry.
fn is_replacement_conflict(stderr: &str) -> bool {
    let conflict = stderr.contains("conflicts with");
    let missing = stderr.contains("is needed by") || stderr.contains("Requires:");
    conflict && !missing
}

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
    use super::is_replacement_conflict;

    #[test]
    fn only_missing_package_requirements_are_resolved() {
        use rpm::Dependency;

        let mut provided = std::collections::BTreeMap::new();
        provided.insert("libfoo.so.1()(64bit)".to_string(), vec![String::new()]);
        provided.insert("libfoo".to_string(), vec!["1.2-3".to_string()]);
        let required = vec![
            Dependency::any("libfoo.so.1()(64bit)"),
            Dependency::greater_eq("libfoo", "1.2"),
            Dependency::greater_eq("libfoo", "2.0"),
            Dependency::any("libbar.so.2()(64bit)"),
            Dependency::any("/usr/bin/sh"),
            Dependency::any("rtld(GNU_HASH)"),
            Dependency::any("rpmlib(CompressedFileNames)"),
            Dependency::any("config(libfoo)"),
        ];
        assert_eq!(
            super::missing_requirements(required, &provided),
            vec!["libfoo >= 2.0".to_string()]
        );
    }

    #[test]
    fn a_replacement_conflict_takes_the_retry_but_a_missing_requirement_does_not() {
        // The zlib example: the test root's gcc pulls zlib-ng-compat, which
        // the package under test replaces.
        assert!(is_replacement_conflict(
            "error: Failed dependencies:\n\
             \tzlib(x86-64) conflicts with (installed) zlib-ng-compat-2.3.3-3.fc44.x86_64"
        ));

        assert!(!is_replacement_conflict(
            "error: Failed dependencies:\n\
             \tlibc.so.6()(64bit) is needed by zlib-1.3.1-1.fc44.x86_64"
        ));

        // Both in one report: the missing dependency is the real problem.
        assert!(!is_replacement_conflict(
            "error: Failed dependencies:\n\
             \tzlib(x86-64) conflicts with (installed) zlib-ng-compat\n\
             \tlibc.so.6()(64bit) is needed by zlib-1.3.1-1.fc44.x86_64"
        ));

        assert!(!is_replacement_conflict(""));
    }
}
