use std::path::{Path, PathBuf};
use std::process::Command;

use rpmx_core::error::codes;
use rpmx_core::profile::load_profile;
use rpmx_core::tree::tree_digest;
use rpmx_core::{
    load_recipe, normalize_dist_tag, plan_with_options, Error, Plan, PlanOptions, Recipe,
};

pub mod build;
pub mod cache;
pub mod fetch;
pub mod lint;
pub mod plan;
pub mod rebuild;
pub mod repo;
pub mod root;
pub mod sign;
pub mod source;
pub mod srpm;

/// Resolve the deterministic build timestamp from the CLI override and the
/// recipe. A missing value stays `0`, which is the historical unsigned default.
pub fn source_date_epoch(recipe: &Recipe, explicit: Option<u32>) -> u32 {
    explicit.or(recipe.package.source_date_epoch).unwrap_or(0)
}

/// Choose the signature creation time. Signed builds must be reproducible, so
/// an explicit timestamp or a non-zero source date epoch is required.
pub fn signature_timestamp(
    explicit: Option<u32>,
    source_date_epoch: u32,
    signing: Option<&rpmx_rpm::sign::SigningKey>,
) -> Result<Option<u32>, Error> {
    let Some(key) = signing else {
        return Ok(None);
    };
    let created_at = key.created_at();
    let timestamp = match explicit {
        Some(seconds) => seconds,
        None if source_date_epoch == 0 => {
            return Err(Error::new(
                codes::SIGN_004,
                "a signed build needs a deterministic timestamp; \
                 set package.source-date-epoch, pass --source-date-epoch, or pass --signing-timestamp",
            ));
        }
        // A key cannot vouch for a time before it existed. Clamp to its
        // creation time so the result stays deterministic and verifiable.
        None => source_date_epoch.max(created_at),
    };
    if timestamp < created_at {
        return Err(Error::new(
            codes::SIGN_004,
            format!(
                "signature timestamp {timestamp} is before signing key {} was created at {created_at}",
                key.fingerprint()
            ),
        ));
    }
    Ok(Some(timestamp))
}

/// Load the optional signing key selected on the command line.
pub fn load_signing_key(
    path: Option<&Path>,
    passphrase_env: Option<&str>,
) -> Result<Option<rpmx_rpm::sign::SigningKey>, Error> {
    let Some(path) = path else {
        return Ok(None);
    };
    let passphrase = signing_passphrase(passphrase_env)?;
    Ok(Some(rpmx_rpm::sign::load_key(path, passphrase.as_deref())?))
}

/// Resolve the passphrase for a signing key from an environment variable.
/// The passphrase is never accepted as a command-line argument.
pub fn signing_passphrase(variable: Option<&str>) -> Result<Option<String>, Error> {
    let Some(variable) = variable else {
        return Ok(None);
    };
    std::env::var(variable).map(Some).map_err(|_| {
        Error::new(
            codes::SIGN_005,
            format!("signing passphrase environment variable {variable} is not set"),
        )
    })
}

pub fn step_root(pipeline_dir: Option<&Path>, recipe: &Path) -> Option<PathBuf> {
    pipeline_dir
        .map(Path::to_path_buf)
        .or_else(|| recipe.parent().map(|parent| parent.join("pipelines")))
}

/// Distribution policy supplied to the planner.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlanPolicy<'a> {
    pub dist_tag: Option<&'a str>,
    pub build_release: Option<u32>,
}

/// Policy inputs as supplied by the command line.
pub struct PolicyInput<'a> {
    pub profile: Option<&'a Path>,
    pub dist_tag: Option<&'a str>,
    pub build_release: Option<u32>,
}

/// Policy after profile defaults have been applied.
#[derive(Debug)]
pub struct ResolvedPolicy {
    pub dist_tag: Option<String>,
    pub build_release: Option<u32>,
}

impl ResolvedPolicy {
    pub fn plan_policy(&self) -> PlanPolicy<'_> {
        PlanPolicy {
            dist_tag: self.dist_tag.as_deref(),
            build_release: self.build_release,
        }
    }
}

/// Apply profile defaults. An explicit `--dist-tag` always wins.
pub fn resolve_policy(input: PolicyInput<'_>) -> Result<ResolvedPolicy, Error> {
    let profile_tag = match input.profile {
        Some(path) => load_profile(path)?.dist_tag()?,
        None => None,
    };
    let dist_tag = match input.dist_tag {
        Some(tag) => Some(normalize_dist_tag(tag)?),
        None => profile_tag,
    };
    Ok(ResolvedPolicy {
        dist_tag,
        build_release: input.build_release,
    })
}

/// Combine an explicit policy with the one recorded inside an SRPM.
///
/// The lock is authoritative: an explicit value may repeat it, but not change
/// it. Old locks without policy fields still deserialize and reconcile, but the
/// rebuild NVR guard rejects any flag that would change their release.
pub fn reconcile_policy<'a>(
    explicit: PlanPolicy<'a>,
    locked: PlanPolicy<'a>,
) -> Result<PlanPolicy<'a>, Error> {
    let dist_tag = match (explicit.dist_tag, locked.dist_tag) {
        (Some(explicit), Some(locked)) => {
            let explicit_tag = normalize_dist_tag(explicit)?;
            let locked_tag = normalize_dist_tag(locked)?;
            if explicit_tag != locked_tag {
                return Err(Error::new(
                    codes::SRPM_012,
                    format!("dist tag '{explicit_tag}' does not match the lock's '{locked_tag}'"),
                ));
            }
            Some(locked)
        }
        (Some(_), None) => explicit.dist_tag,
        (None, locked) => locked,
    };
    let build_release = match (explicit.build_release, locked.build_release) {
        (Some(explicit), Some(locked)) if explicit != locked => {
            return Err(Error::new(
                codes::SRPM_012,
                format!("build release {explicit} does not match the lock's {locked}"),
            ));
        }
        (Some(_), Some(locked)) => Some(locked),
        (Some(explicit), None) => Some(explicit),
        (None, locked) => locked,
    };
    Ok(PlanPolicy {
        dist_tag,
        build_release,
    })
}

#[cfg(test)]
mod tests {
    use super::{reconcile_policy, resolve_policy, PlanPolicy, PolicyInput};

    const PROFILE: &str = r#"
document: rpmx-profile
version: 1
name: test
releasever: "44"
dist-tag: .fc44
archs: [x86_64]
repositories:
  - id: fedora
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
"#;

    #[test]
    fn profile_supplies_the_dist_tag_and_cli_overrides_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("profile.yaml");
        std::fs::write(&path, PROFILE).expect("profile");

        let from_profile = resolve_policy(PolicyInput {
            profile: Some(&path),
            dist_tag: None,
            build_release: None,
        })
        .expect("profile policy");
        assert_eq!(from_profile.dist_tag.as_deref(), Some(".fc44"));

        let overridden = resolve_policy(PolicyInput {
            profile: Some(&path),
            dist_tag: Some("fc45"),
            build_release: Some(3),
        })
        .expect("override policy");
        assert_eq!(overridden.dist_tag.as_deref(), Some(".fc45"));
        assert_eq!(overridden.build_release, Some(3));
    }

    #[test]
    fn profile_with_an_invalid_dist_tag_is_rejected() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("profile.yaml");
        std::fs::write(&path, PROFILE.replace(".fc44", "fc/44")).expect("profile");
        let error = resolve_policy(PolicyInput {
            profile: Some(&path),
            dist_tag: None,
            build_release: None,
        })
        .expect_err("invalid profile tag");
        assert_eq!(error.code, "RPMX-ROOT-006");
    }

    #[test]
    fn lock_policy_is_authoritative() {
        let policy = reconcile_policy(
            PlanPolicy::default(),
            PlanPolicy {
                dist_tag: Some(".fc44"),
                build_release: Some(7),
            },
        )
        .expect("locked policy");
        assert_eq!(policy.dist_tag, Some(".fc44"));
        assert_eq!(policy.build_release, Some(7));
    }

    #[test]
    fn explicit_policy_may_repeat_the_lock_in_another_form() {
        let policy = reconcile_policy(
            PlanPolicy {
                dist_tag: Some("fc44"),
                build_release: Some(7),
            },
            PlanPolicy {
                dist_tag: Some(".fc44"),
                build_release: Some(7),
            },
        )
        .expect("matching policy");
        assert_eq!(policy.dist_tag, Some(".fc44"));
    }

    #[test]
    fn explicit_policy_must_not_change_the_lock() {
        let error = reconcile_policy(
            PlanPolicy {
                dist_tag: Some("fc45"),
                build_release: Some(8),
            },
            PlanPolicy {
                dist_tag: Some(".fc44"),
                build_release: Some(7),
            },
        )
        .expect_err("mismatch");
        assert_eq!(error.code, "RPMX-SRPM-012");
        assert!(error.message.contains("dist tag"));
    }

    #[test]
    fn build_release_must_not_change_the_lock() {
        let error = reconcile_policy(
            PlanPolicy {
                dist_tag: None,
                build_release: Some(8),
            },
            PlanPolicy {
                dist_tag: None,
                build_release: Some(7),
            },
        )
        .expect_err("mismatch");
        assert_eq!(error.code, "RPMX-SRPM-012");
        assert!(error.message.contains("build release"));
    }

    #[test]
    fn legacy_locks_reconcile_explicit_policy() {
        let policy = reconcile_policy(
            PlanPolicy {
                dist_tag: Some("fc44"),
                build_release: Some(7),
            },
            PlanPolicy::default(),
        )
        .expect("legacy lock");
        assert_eq!(policy.dist_tag, Some("fc44"));
        assert_eq!(policy.build_release, Some(7));
    }
}

pub fn load_plan(
    file: &Path,
    arch: &str,
    pipeline_dir: Option<&Path>,
    allow_builtin_override: bool,
    policy: PlanPolicy<'_>,
) -> Result<(Recipe, Plan), Error> {
    let recipe = load_recipe(file)?;
    let root = step_root(pipeline_dir, file);
    let plan = plan_with_options(
        &recipe,
        PlanOptions {
            arch,
            step_root: root.as_deref(),
            allow_builtin_override,
            dist_tag: policy.dist_tag,
            build_release: policy.build_release,
        },
    )?;
    Ok((recipe, plan))
}

pub struct SrpmArgs<'a> {
    pub file: &'a Path,
    pub sources: &'a Path,
    pub output: &'a Path,
    pub arch: &'a str,
    pub source_date_epoch: Option<u32>,
    pub cell: Option<&'a str>,
    pub pipeline_dir: Option<&'a Path>,
    pub policy: PlanPolicy<'a>,
    pub root_lock: Option<&'a Path>,
    pub signing_key: Option<&'a Path>,
    pub signing_passphrase_env: Option<&'a str>,
    pub signing_timestamp: Option<u32>,
    pub quiet: bool,
    pub json: bool,
}

pub struct BuildArgs<'a> {
    pub file: &'a Path,
    pub stage: Option<&'a Path>,
    pub output: &'a Path,
    pub arch: &'a str,
    pub source_date_epoch: Option<u32>,
    pub cell: Option<&'a str>,
    pub pipeline_dir: Option<&'a Path>,
    pub policy: PlanPolicy<'a>,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub unsafe_host_root: bool,
    pub jobs: u32,
    pub step_timeout: u64,
    pub run_tests: bool,
    pub sbom: Option<&'a Path>,
    pub test_root_lock: Option<&'a Path>,
    pub signing_key: Option<&'a Path>,
    pub signing_passphrase_env: Option<&'a str>,
    pub signing_timestamp: Option<u32>,
    pub json: bool,
}

pub struct RebuildArgs<'a> {
    pub file: &'a Path,
    pub output: &'a Path,
    pub arch: &'a str,
    pub cell: Option<&'a str>,
    pub policy: PlanPolicy<'a>,
    pub jobs: u32,
    pub step_timeout: u64,
    pub root_image: Option<&'a str>,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub run_tests: bool,
    pub sbom: Option<&'a Path>,
    pub test_root_lock: Option<&'a Path>,
    pub signing_key: Option<&'a Path>,
    pub signing_passphrase_env: Option<&'a str>,
    pub signing_timestamp: Option<u32>,
    pub quiet: bool,
    pub json: bool,
}

pub struct FetchArgs<'a> {
    pub file: &'a Path,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub json: bool,
}

pub struct SourceArgs<'a> {
    pub file: &'a Path,
    pub pipeline_dir: Option<&'a Path>,
    pub policy: PlanPolicy<'a>,
    pub cache: Option<&'a Path>,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub output: &'a Path,
    pub source_date_epoch: Option<u32>,
    pub jobs: u32,
    pub step_timeout: u64,
    pub root_lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub unsafe_host_root: bool,
    pub quiet: bool,
    pub json: bool,
}

pub struct RootResolveArgs<'a> {
    pub profile: &'a Path,
    pub output: Option<&'a Path>,
    pub work: Option<&'a Path>,
    pub keep: bool,
    pub cache: Option<&'a Path>,
    pub json: bool,
}

pub struct RootAssembleArgs<'a> {
    pub lock: &'a Path,
    pub root: Option<&'a Path>,
    pub output: Option<&'a Path>,
    pub cache: Option<&'a Path>,
    pub work: Option<&'a Path>,
    pub keep: bool,
    pub json: bool,
}

pub struct RootDigestArgs<'a> {
    pub root: &'a Path,
    pub json: bool,
}

pub struct RootExportArgs<'a> {
    pub lock: &'a Path,
    pub output: Option<&'a Path>,
}

pub struct RootImportArgs<'a> {
    pub manifest: &'a Path,
    pub profile: &'a str,
    pub arch: Option<&'a str>,
    pub releasever: &'a str,
    pub output: Option<&'a Path>,
}

/// Compute a root digest, re-entering the user namespace when package files are
/// not readable by the caller.
pub(crate) fn digest_root(root: &Path) -> Result<String, Error> {
    match tree_digest(root) {
        Ok(digest) => Ok(digest),
        Err(direct) => {
            let executable = std::env::current_exe().map_err(|error| {
                Error::new(
                    codes::ROOT_014,
                    format!("cannot locate this executable: {error}"),
                )
            })?;
            // util-linux first: it does not need podman. Fall back to podman's
            // user namespace when the kernel or container policy blocks it.
            let attempts = [
                Command::new("unshare")
                    .arg("-Ur")
                    .arg("--")
                    .arg(&executable)
                    .arg("root")
                    .arg("digest")
                    .arg(root)
                    .output(),
                Command::new("podman")
                    .arg("unshare")
                    .arg("--")
                    .arg(&executable)
                    .arg("root")
                    .arg("digest")
                    .arg(root)
                    .output(),
            ];
            for output in attempts.into_iter().flatten() {
                if output.status.success() {
                    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if text.starts_with("sha512:") {
                        return Ok(text);
                    }
                }
            }
            Err(direct)
        }
    }
}
