//! Read a recipe's declared root policy without acquiring or assembling roots.
mod planning;
pub mod policy;
pub use planning::{load_plan, step_root};

use crate::profile::{
    load_profile, Builder, Packages, Profile, Repository, ResolveOptions, DOCUMENT, VERSION,
};
use rpmx_base::{error::codes, hash::sha512_hex, Error};
use rpmx_language::{
    model::{Recipe, RootRepository, RootSpec},
    plan::Plan,
};
use std::path::{Path, PathBuf};

/// The merged root requirement for a recipe: profile, name and digest.
#[derive(Debug)]
pub struct Requirement {
    /// The root the steps run in: the build root for a cross profile.
    pub profile: Profile,
    pub name: String,
    pub digest: String,
    /// The target sysroot, when the profile cross-compiles.
    pub sysroot: Option<SysrootRequirement>,
    /// Toolchain commands for a cross build.
    pub cross: Option<rpmx_base::guest::CrossToolchain>,
}

/// The target half of a cross build: a second closure with its own identity.
#[derive(Debug)]
pub struct SysrootRequirement {
    pub profile: Profile,
    pub name: String,
    pub digest: String,
}

/// Inspect a verified assembled cache entry without resolving or acquiring anything.
pub fn cached_lock_for_requirement(cache: &Path, digest: &str) -> Option<crate::root::RootLock> {
    let roots = cache.parent().unwrap_or(cache).join("roots");
    let key = digest.strip_prefix("sha512:").unwrap_or(digest);
    // The digest is a directory component, never a caller-selected path.
    if key.len() != 128 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let entry = roots.join(key);
    let lock = crate::root::load_lock(&entry.join("root.lock.yaml")).ok()?;
    crate::root::RootSelection::verify(&lock, &entry.join("root"), lock.archs.first()?).ok()?;
    Some(lock)
}

/// The distribution tag declared by a recipe's named root profile.
pub fn profile_dist_tag(recipe: &Recipe, recipe_path: &Path) -> Result<Option<String>, Error> {
    let Some(spec) = &recipe.build.root else {
        return Ok(None);
    };
    let Some(name) = &spec.profile else {
        return Ok(None);
    };
    let recipe_dir = recipe_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let path = discover_profile(name, recipe_dir)?;
    load_profile(&path)?.dist_tag()
}

/// The single target a recipe's profile declares, if exactly one.
///
/// A cross profile names one target; commands that would otherwise default to
/// the host architecture use this so the recipe builds what it says.
pub fn declared_arch(
    recipe: &Recipe,
    recipe_path: &Path,
    profile: Option<&Path>,
) -> Result<Option<String>, Error> {
    Ok(declared_arches(recipe, recipe_path, profile)?
        .filter(|arches| arches.len() == 1)
        .map(|arches| arches[0].clone()))
}

/// Allowed targets from the selected named profile, without preparing any root.
pub fn declared_arches(
    recipe: &Recipe,
    recipe_path: &Path,
    profile: Option<&Path>,
) -> Result<Option<Vec<String>>, Error> {
    let path = match profile {
        Some(path) => Some(path.to_path_buf()),
        None => {
            let Some(spec) = &recipe.build.root else {
                return Ok(None);
            };
            let Some(name) = &spec.profile else {
                return Ok(None);
            };
            let directory = recipe_path.parent().unwrap_or_else(|| Path::new("."));
            Some(discover_profile(name, directory)?)
        }
    };
    let Some(path) = path else {
        return Ok(None);
    };
    let profile = load_profile(&path)?;
    Ok(Some(profile.archs))
}

/// Build the merged root requirement without touching the network or cache.
///
/// Returns `None` when the recipe names no profile and the caller gave none.
pub fn requirement(
    recipe: &Recipe,
    plan: &Plan,
    recipe_path: &Path,
    profile: Option<&Path>,
    arch: &str,
) -> Result<Option<Requirement>, Error> {
    let spec = recipe.build.root.as_ref();
    let selected_profile = profile.map(Path::to_path_buf);
    if spec.is_none() && selected_profile.is_none() {
        return Ok(None);
    }

    let recipe_dir = recipe_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut profile = match (spec, selected_profile.as_deref()) {
        // An explicit --profile is the caller's last word over the recipe's
        // named profile. Silently preferring the recipe would also mix its
        // root with the flag's policy, which is worse than either.
        (Some(spec), Some(path)) => {
            if spec.profile.is_some() && has_inline_fields(spec) {
                return Err(Error::new(
                    codes::ROOT_006,
                    "build.root must name a profile or describe one inline, not both",
                )
                .at("build.root"));
            }
            if has_inline_fields(spec) {
                return Err(Error::new(
                    codes::ROOT_006,
                    "an inline build.root cannot be combined with --profile; remove one",
                )
                .at("build.root"));
            }
            load_profile(path)?
        }
        (Some(spec), None) if spec.profile.is_some() => {
            if has_inline_fields(spec) {
                return Err(Error::new(
                    codes::ROOT_006,
                    "build.root must name a profile or describe one inline, not both",
                )
                .at("build.root"));
            }
            let path = discover_profile(spec.profile.as_deref().unwrap_or_default(), recipe_dir)?;
            let profile = load_profile(&path)?;
            reject_recipe_profile_credentials(&profile, &path)?;
            profile
        }
        (Some(spec), None) => inline_profile(spec, arch)?,
        (None, Some(path)) => load_profile(path)?,
        (None, None) => unreachable!("guarded above"),
    };

    let required = rpmx_language::plan::build_requirements(plan);
    let crossing = profile.cross.is_some();
    if crossing {
        // The build root takes the tools; the target keeps its libraries.
        for package in &required.sysroot {
            push_unique(&mut profile.packages.install, package);
        }
        if let Some(cross) = profile.cross.as_mut() {
            for package in &required.packages {
                push_unique(&mut cross.packages, package);
            }
            cross.packages.sort();
        }
    } else {
        for package in required.packages.iter().chain(required.sysroot.iter()) {
            push_unique(&mut profile.packages.install, package);
        }
    }
    // Inline roots never pass through `load_profile`; validate the assembled
    // profile before anything derives an architecture or a root path from it.
    profile.validate()?;
    // `archs` is an allowed set. A root is resolved for exactly one target.
    let profile = profile.for_target(arch)?;
    let cross = if crossing {
        let declared = profile.cross().expect("crossing implies a cross block");
        let triple = profile.triple()?;
        // A native bootstrap may use the seed's native command names when the
        // profile does not name a triple. An explicit triple always selects
        // the GNU `<triple>-*` wrappers, which the ladder's cross toolchain
        // needs for target CRT and libgcc discovery.
        let mut toolchain = if declared.bootstrap && declared.triple.is_none() {
            rpmx_base::guest::CrossToolchain::native(triple)
        } else {
            rpmx_base::guest::CrossToolchain::new(triple)
        };
        toolchain.cflags = declared.cflags.clone();
        toolchain.cxxflags = declared.cxxflags.clone();
        toolchain.ldflags = declared.ldflags.clone();
        Some(toolchain)
    } else {
        None
    };

    let (primary, sysroot) = if crossing {
        let sysroot = profile.sysroot_profile()?;
        (profile.build_root_profile()?, Some(sysroot))
    } else {
        (profile, None)
    };

    let fallback = format!("{}-root", recipe.package.name);
    let name = primary.name(&fallback);
    let digest = digest_profile(&primary)?;
    let sysroot = match sysroot {
        Some(sysroot) => Some(SysrootRequirement {
            name: sysroot.name(&fallback),
            digest: digest_profile(&sysroot)?,
            profile: sysroot,
        }),
        None => None,
    };
    // The build-root requirement is runner-independent. The QEMU appliance is
    // a runner asset, resolved and cached separately from this identity.
    Ok(Some(Requirement {
        profile: primary,
        name,
        digest,
        sysroot,
        cross,
    }))
}

fn digest_profile(profile: &Profile) -> Result<String, Error> {
    let bytes = serde_json::to_vec(profile)
        .map_err(|error| Error::new(codes::ROOT_006, error.to_string()))?;
    Ok(sha512_hex(&bytes))
}

fn push_unique(packages: &mut Vec<String>, package: &String) {
    if !packages.contains(package) {
        packages.push(package.clone());
    }
}

fn has_inline_fields(spec: &RootSpec) -> bool {
    spec.releasever.is_some()
        || !spec.archs.is_empty()
        || !spec.repositories.is_empty()
        || !spec.packages.is_empty()
        || spec.builder.is_some()
}

fn inline_profile(spec: &RootSpec, arch: &str) -> Result<Profile, Error> {
    let releasever = spec.releasever.clone().ok_or_else(|| {
        Error::new(
            codes::ROOT_006,
            "an inline build.root needs releasever, repositories and packages",
        )
        .at("build.root.releasever")
    })?;
    if spec.repositories.is_empty() {
        return Err(Error::new(
            codes::ROOT_006,
            "an inline build.root needs at least one repository",
        )
        .at("build.root.repositories"));
    }
    let profile = Profile {
        document: DOCUMENT.to_string(),
        version: VERSION,
        name: None,
        releasever,
        dist_tag: None,
        archs: if spec.archs.is_empty() {
            vec![arch.to_string()]
        } else {
            spec.archs.clone()
        },
        builder: Builder {
            image: spec.builder.as_ref().map(|builder| builder.image.clone()),
        },
        repositories: spec.repositories.iter().map(repository).collect(),
        packages: Packages {
            install: spec.packages.clone(),
            reinstall: Vec::new(),
        },
        // An inline root describes one closure; cross needs a profile because
        // it names a second, host-architecture package set.
        cross: None,
        modules: Default::default(),
        options: ResolveOptions::default(),
        paths: Default::default(),
        sbom: true,
    };
    profile.for_target(arch)
}

fn repository(source: &RootRepository) -> Repository {
    Repository {
        id: source.id.clone(),
        baseurl: source.baseurl.clone(),
        metalink: source.metalink.clone(),
        mirrorlist: source.mirrorlist.clone(),
        gpgkey: source.gpgkey.clone(),
        repo_gpgcheck: source.repo_gpgcheck,
        exclude: Vec::new(),
        priority: None,
        auth: None,
    }
}

/// Find a profile by path, then by convention.
fn discover_profile(name: &str, recipe_dir: &Path) -> Result<PathBuf, Error> {
    if name.is_empty() {
        return Err(Error::new(codes::ROOT_006, "build.root.profile is empty"));
    }
    // A relative path is relative to the recipe, not the caller's cwd.
    let relative = recipe_dir.join(name);
    if relative.is_file() {
        return Ok(relative);
    }
    let explicit = Path::new(name);
    if explicit.is_file() {
        return Ok(explicit.to_path_buf());
    }
    let mut candidates = vec![
        recipe_dir.join("profiles").join(format!("{name}.yaml")),
        recipe_dir
            .join("profiles")
            .join(format!("{name}.profile.yaml")),
    ];
    if let Some(config) = config_dir() {
        candidates.push(config.join("profiles").join(format!("{name}.yaml")));
    }
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    Err(Error::new(
        codes::ROOT_006,
        format!(
            "profile '{name}' not found; looked at {}",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ))
}

fn config_dir() -> Option<PathBuf> {
    if let Ok(base) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(base).join("rpmx"));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config").join("rpmx"))
}

/// A recipe chooses which profile file it loads. That file may contain
/// repository credentials, so only a profile the user controls may carry them.
fn reject_recipe_profile_credentials(profile: &Profile, path: &Path) -> Result<(), Error> {
    if !profile.repositories.iter().any(|repo| repo.auth.is_some()) {
        return Ok(());
    }
    let trusted = config_dir()
        .map(|config| path_is_under(path, &config.join("profiles")))
        .unwrap_or(false);
    if trusted {
        return Ok(());
    }
    Err(Error::new(
        codes::ROOT_019,
        format!(
            "profile {} was selected by the recipe and carries repository credentials; \
             pass --profile explicitly or place it under {}/profiles",
            path.display(),
            config_dir()
                .map(|config| config.display().to_string())
                .unwrap_or_else(|| "$XDG_CONFIG_HOME/rpmx".to_string())
        ),
    ))
}

/// Compare resolved paths: a lexical `starts_with` would accept `profiles/../..`.
fn path_is_under(path: &Path, root: &Path) -> bool {
    let Ok(path) = std::fs::canonicalize(path) else {
        return false;
    };
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::has_inline_fields;
    use rpmx_language::Recipe;
    fn recipe(text: &str) -> Recipe {
        rpmx_language::parse_recipe(text).expect("recipe")
    }

    fn plan_for(recipe: &Recipe, arch: &str) -> rpmx_language::Plan {
        rpmx_language::plan_with_options(
            recipe,
            rpmx_language::PlanOptions {
                arch,
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan")
    }

    fn plan(recipe: &Recipe) -> rpmx_language::Plan {
        plan_for(recipe, "x86_64")
    }

    #[test]
    fn declared_targets_follow_recipe_profile_and_explicit_override() {
        let directory = tempfile::tempdir().unwrap();
        let profile = "document: rpmx-profile\nversion: 1\nname: targets\nreleasever: '44'\narchs: [x86_64, aarch64]\nrepositories:\n  - id: local\n    baseurl: https://example.invalid/repo\npackages:\n  install: [bash]\n";
        std::fs::write(directory.path().join("targets.yaml"), profile).unwrap();
        let single = directory.path().join("single.yaml");
        std::fs::write(&single, profile.replace("[x86_64, aarch64]", "[aarch64]")).unwrap();
        let recipe = recipe("schema: 2\npackage:\n  name: targets\n  version: '1'\n  release: 1\n  summary: targets\n  license: MIT\nbuild:\n  root:\n    profile: targets.yaml\noutputs:\n  - name: targets\n    metapackage: true\n");
        let path = directory.path().join("package.yaml");
        assert_eq!(
            super::declared_arches(&recipe, &path, None)
                .unwrap()
                .unwrap(),
            ["x86_64", "aarch64"]
        );
        assert_eq!(super::declared_arch(&recipe, &path, None).unwrap(), None);
        assert_eq!(
            super::declared_arch(&recipe, &path, Some(&single))
                .unwrap()
                .as_deref(),
            Some("aarch64")
        );
    }

    #[test]
    fn cache_inspection_never_accepts_a_path_as_a_requirement_digest() {
        let directory = tempfile::tempdir().unwrap();
        for digest in ["", "../outside", "/absolute", "sha512:../outside"] {
            assert!(super::cached_lock_for_requirement(directory.path(), digest).is_none());
        }
        let digest = format!("sha512:{}", "a".repeat(128));
        assert!(super::cached_lock_for_requirement(directory.path(), &digest).is_none());
    }

    #[test]
    fn a_profile_name_and_inline_fields_are_mutually_exclusive() {
        let recipe = recipe(
            r#"
schema: 2
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    profile: base
    releasever: "44"
outputs:
  - name: p
    metapackage: true
"#,
        );
        let spec = recipe.build.root.expect("root");
        assert!(has_inline_fields(&spec));
    }

    #[test]
    fn examples_name_their_own_build_root() {
        let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let mut checked = 0;
        for entry in std::fs::read_dir(examples).expect("examples directory") {
            let path = entry.expect("entry").path().join("package.yaml");
            let Ok(bundle) = rpmx_language::load_recipe_bundle(&path) else {
                // curl and libfoo are design artifacts for fields that do not
                // exist yet; they cannot build and are not covered here.
                continue;
            };
            // A cross example declares exactly one target; build that one.
            let mut arch = rpmx_base::arch::native_arch().to_string();
            if let Some(spec) = &bundle.recipe.build.root {
                if let Some(name) = &spec.profile {
                    let directory = bundle
                        .root
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."));
                    if let Ok(profile_path) = super::discover_profile(name, directory) {
                        if let Ok(profile) = crate::profile::load_profile(&profile_path) {
                            if profile.archs.len() == 1 {
                                arch = profile.archs[0].clone();
                            }
                        }
                    }
                }
            }
            let plan = plan_for(&bundle.recipe, &arch);
            let root = super::requirement(&bundle.recipe, &plan, &bundle.root, None, &arch)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert!(
                root.is_some(),
                "{} declares no build.root; examples must build with `rpmx build FILE`",
                path.display()
            );
            checked += 1;
        }
        assert!(checked >= 10, "only checked {checked} examples");
    }

    #[test]
    fn an_inline_root_derives_the_requested_architecture() {
        let directory = tempfile::tempdir().expect("tempdir");
        let recipe = recipe(
            r#"
schema: 2
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    releasever: "44"
    archs: [x86_64, aarch64]
    repositories:
      - id: fedora
        baseurl: https://example.invalid/fedora
    packages: [bash]
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let path = directory.path().join("package.yaml");
        let x86 = super::requirement(&recipe, &plan, &path, None, "x86_64")
            .expect("requirement")
            .expect("root");
        let arm = super::requirement(&recipe, &plan, &path, None, "aarch64")
            .expect("requirement")
            .expect("root");
        assert_eq!(x86.profile.archs, vec!["x86_64".to_string()]);
        assert_eq!(arm.profile.archs, vec!["aarch64".to_string()]);
        assert_ne!(x86.digest, arm.digest);
    }

    #[test]
    fn a_cross_profile_resolves_a_build_root_and_a_sysroot() {
        let directory = tempfile::tempdir().expect("tempdir");
        let native = rpmx_base::Arch::current().expect("host").as_str();
        let target = if native == "x86_64" {
            "aarch64"
        } else {
            "x86_64"
        };
        let profile_path = directory.path().join("cross.yaml");
        std::fs::write(
            &profile_path,
            format!(
                r#"
document: rpmx-profile
version: 1
name: cross
releasever: "44"
archs: [{target}]
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
cross:
  packages: [toolchain-cc]
"#
            ),
        )
        .expect("profile");

        let recipe = recipe(
            r#"
schema: 2
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  packages: [make]
  sysroot: [zlib-devel]
  pipeline:
    - uses: strip
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let path = directory.path().join("package.yaml");
        let requirement = super::requirement(&recipe, &plan, &path, Some(&profile_path), target)
            .expect("requirement")
            .expect("root");

        // The root the steps run in is native and holds the toolchain.
        assert_eq!(requirement.profile.archs, vec![native.to_string()]);
        assert!(requirement
            .profile
            .packages
            .install
            .contains(&"toolchain-cc".to_string()));
        assert!(requirement
            .profile
            .packages
            .install
            .contains(&"make".to_string()));
        assert!(
            !requirement
                .profile
                .packages
                .install
                .contains(&"zlib-devel".to_string()),
            "a target library cannot live in the host root"
        );

        // The sysroot is the target closure with its libraries.
        let sysroot = requirement.sysroot.expect("sysroot");
        assert_eq!(sysroot.profile.archs, vec![target.to_string()]);
        assert!(sysroot
            .profile
            .packages
            .install
            .contains(&"zlib-devel".to_string()));
        assert!(sysroot
            .profile
            .packages
            .install
            .contains(&"bash".to_string()));
        assert_ne!(
            requirement.digest, sysroot.digest,
            "the two roots need distinct cache identities"
        );
    }

    #[test]
    fn a_native_bootstrap_resolves_two_roots_with_native_tools() {
        let directory = tempfile::tempdir().expect("tempdir");
        let native = rpmx_base::Arch::current().expect("host").as_str();
        let profile_path = directory.path().join("bootstrap.yaml");
        std::fs::write(
            &profile_path,
            format!(
                r#"
document: rpmx-profile
version: 1
name: bootstrap
releasever: "44"
archs: [{native}]
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [glibc, glibc-devel]
cross:
  host: {native}
  bootstrap: true
  packages: [seed-gcc, seed-make]
"#
            ),
        )
        .expect("profile");

        let recipe = recipe(
            r#"
schema: 2
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  packages: [make]
  sysroot: [zlib-devel]
  pipeline:
    - uses: strip
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let path = directory.path().join("package.yaml");
        let requirement = super::requirement(&recipe, &plan, &path, Some(&profile_path), native)
            .expect("requirement")
            .expect("root");

        let toolchain = requirement.cross.as_ref().expect("toolchain");
        assert!(toolchain.native, "no triple means native command names");
        assert_eq!(toolchain.cc, "gcc");
        assert_eq!(
            toolchain.triple,
            rpmx_base::Arch::parse(native).unwrap().triple()
        );

        assert_eq!(requirement.profile.archs, vec![native.to_string()]);
        assert!(requirement
            .profile
            .packages
            .install
            .contains(&"seed-gcc".to_string()));
        assert!(requirement
            .profile
            .packages
            .install
            .contains(&"make".to_string()));
        assert!(!requirement
            .profile
            .packages
            .install
            .contains(&"zlib-devel".to_string()));

        let sysroot = requirement.sysroot.expect("sysroot");
        assert_eq!(sysroot.profile.archs, vec![native.to_string()]);
        assert!(sysroot
            .profile
            .packages
            .install
            .contains(&"zlib-devel".to_string()));
        assert!(!sysroot
            .profile
            .packages
            .install
            .contains(&"seed-gcc".to_string()));
        assert_ne!(
            requirement.digest, sysroot.digest,
            "the two roots need distinct cache identities"
        );
    }

    #[test]
    fn inline_roots_are_validated_before_any_resolution() {
        let directory = tempfile::tempdir().expect("tempdir");
        let recipe = recipe(
            r#"
schema: 2
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    releasever: "44"
    archs: [typo]
    repositories:
      - id: repo
        baseurl: https://example.invalid/fedora
    packages: [bash]
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let error = super::requirement(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            "x86_64",
        )
        .expect_err("invalid arch");
        // `Profile::validate` reports an unknown architecture as ROOT-009.
        assert_eq!(error.code, rpmx_base::error::codes::ROOT_009);
    }

    #[test]
    fn naming_a_profile_and_describing_one_is_rejected() {
        let directory = tempfile::tempdir().expect("tempdir");
        let recipe = recipe(
            r#"
schema: 2
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    profile: base
    releasever: "44"
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let error = super::requirement(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            "x86_64",
        )
        .expect_err("conflicting root");
        assert_eq!(error.code, rpmx_base::error::codes::ROOT_006);
        assert_eq!(error.path.as_deref(), Some("build.root"));
    }

    #[test]
    fn profile_trust_rejects_traversal_and_symlinks() {
        let directory = tempfile::tempdir().expect("tempdir");
        let trusted = directory.path().join("config/profiles");
        std::fs::create_dir_all(&trusted).expect("profiles");
        let inside = trusted.join("review.yaml");
        std::fs::write(&inside, b"profile").expect("inside");
        assert!(super::path_is_under(&inside, &trusted));

        let outside = directory.path().join("outside.yaml");
        std::fs::write(&outside, b"profile").expect("outside");
        let traversal = trusted.join("../../outside.yaml");
        assert!(!super::path_is_under(&traversal, &trusted));

        #[cfg(unix)]
        {
            let link = trusted.join("linked.yaml");
            std::os::unix::fs::symlink(&outside, &link).expect("symlink");
            assert!(!super::path_is_under(&link, &trusted));
        }
    }

    #[test]
    fn a_recipe_selected_profile_cannot_carry_credentials() {
        let directory = tempfile::tempdir().expect("tempdir");
        let profiles = directory.path().join("profiles");
        std::fs::create_dir_all(&profiles).expect("profiles");
        std::fs::write(
            profiles.join("review.yaml"),
            r#"
document: rpmx-profile
version: 1
name: review
releasever: "44"
archs: [x86_64]
repositories:
  - id: private
    baseurl: https://example.invalid/private
    auth:
      username: builder
      password: env:RPMX_TEST_RECIPE_PASSWORD
packages:
  install: [bash]
"#,
        )
        .expect("profile");
        let recipe = recipe(
            r#"
schema: 2
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    profile: profiles/review.yaml
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let error = super::requirement(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            "x86_64",
        )
        .expect_err("credentials");
        assert_eq!(error.code, rpmx_base::error::codes::ROOT_019);
        assert!(
            error.message.contains("pass --profile"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_profile_path_is_relative_to_the_recipe() {
        let directory = tempfile::tempdir().expect("tempdir");
        let profiles = directory.path().join("profiles");
        std::fs::create_dir_all(&profiles).expect("profiles");
        std::fs::write(
            profiles.join("review.yaml"),
            r#"
document: rpmx-profile
version: 1
name: review
releasever: "44"
archs: [x86_64]
repositories:
  - id: fedora
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
"#,
        )
        .expect("profile");
        let recipe = recipe(
            r#"
schema: 2
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  root:
    profile: profiles/review.yaml
outputs:
  - name: p
    metapackage: true
"#,
        );
        let plan = plan(&recipe);
        let selected = super::requirement(
            &recipe,
            &plan,
            &directory.path().join("package.yaml"),
            None,
            "x86_64",
        )
        .expect("relative profile")
        .expect("requirement");
        assert_eq!(selected.name, "review");
        assert_eq!(selected.profile.packages.install, ["bash"]);
    }

    #[test]
    fn plan_dependencies_and_step_needs_join_the_root() {
        let recipe = recipe(
            r#"
schema: 2
package:
  name: p
  version: "1"
  release: 1
  summary: p
  license: MIT
build:
  packages: [gcc]
  sysroot: [zlib-devel]
  pipeline:
    - uses: strip
  tests:
    - name: upstream
      packages: [dejagnu]
      pipeline:
        - uses: make/check
outputs:
  - name: p
    metapackage: true
tests:
  - name: installed
    packages: [pytest]
    pipeline:
      - runs: "true"
"#,
        );
        let plan = rpmx_language::plan_with_options(
            &recipe,
            rpmx_language::PlanOptions {
                arch: "x86_64",
                step_root: None,
                allow_builtin_override: false,
                dist_tag: None,
                build_release: None,
            },
        )
        .expect("plan");
        let packages = rpmx_language::plan::build_requirements(&plan);
        // Both kinds of test tools have separate environments and must
        // not enter either production build closure.
        assert!(packages.packages.contains("gcc"));
        assert!(!packages.sysroot.contains("gcc"));
        assert!(packages.sysroot.contains("zlib-devel"));
        assert!(!packages.sysroot.contains("pytest"));
        assert!(!packages.packages.contains("pytest"));
        assert!(!packages.packages.contains("dejagnu"));
        assert!(!packages.packages.contains("make"));
        assert!(!packages.sysroot.contains("dejagnu"));
        // A built-in step contributes its tools to the build root.
        assert!(packages.packages.contains("binutils"));
        assert!(packages.packages.contains("findutils"));
    }
}
