use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::profile::load_profile;
use rpmx_core::{normalize_dist_tag, Error};

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
