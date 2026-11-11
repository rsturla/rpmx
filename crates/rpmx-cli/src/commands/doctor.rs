//! Check the host features rpmx depends on.

use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

use crate::commands::DoctorArgs;

#[derive(Debug, Clone, Serialize)]
struct Check {
    name: &'static str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

pub fn run(args: DoctorArgs) -> Result<(), rpmx_core::Error> {
    let mut checks = Vec::new();
    let mut missing_core = false;
    let bubblewrap = on_path("bwrap").is_some();

    for (name, required) in [
        ("dnf5", false),
        ("tar", true),
        ("git", false),
        ("ssh", false),
        ("ssh-keygen", false),
        ("mke2fs", false),
        ("debugfs", false),
        ("unshare", bubblewrap),
        ("newuidmap", bubblewrap),
        ("newgidmap", bubblewrap),
    ] {
        let path = on_path(name);
        let check = match path {
            Some(path) => Check {
                name,
                status: "ok",
                version: version(name),
                detail: Some(path.display().to_string()),
            },
            None => {
                if required {
                    missing_core = true;
                }
                Check {
                    name,
                    status: if required { "fail" } else { "warn" },
                    version: None,
                    detail: Some(if required {
                        "required".to_string()
                    } else {
                        "optional for this runner".to_string()
                    }),
                }
            }
        };
        checks.push(check);
    }

    if bubblewrap {
        let namespace = rpmx_core::runner::check_bwrap_environment();
        checks.push(Check {
            name: "bubblewrap environment",
            status: if namespace.is_ok() { "ok" } else { "fail" },
            version: None,
            detail: Some(match namespace {
                Ok(()) => {
                    "UID/GID 65534 mapped to caller; full subordinate ID maps; nested user namespaces disabled; private transaction mode-000 read/write verified".to_string()
                }
                Err(error) => error.to_string(),
            }),
        });
    }

    for descriptor in rpmx_core::runner::RunnerKind::descriptors() {
        let found = descriptor
            .tools
            .iter()
            .find(|tool| on_path(tool).is_some())
            .copied();
        checks.push(match found {
            Some(tool) => Check {
                name: descriptor.name,
                status: "ok",
                version: version(tool),
                detail: Some(tool.to_string()),
            },
            None => Check {
                name: descriptor.name,
                status: "warn",
                version: None,
                detail: Some(if descriptor.assets.is_empty() {
                    "optional runner".to_string()
                } else {
                    "optional for foreign-architecture builds".to_string()
                }),
            },
        });
    }

    if on_path("bwrap").is_none() && on_path("podman").is_none() {
        missing_core = true;
        checks.push(Check {
            name: "runner",
            status: "fail",
            version: None,
            detail: Some("install bubblewrap or podman".to_string()),
        });
    } else {
        checks.push(Check {
            name: "runner",
            status: "ok",
            version: None,
            detail: Some(if on_path("bwrap").is_some() {
                "bubblewrap".to_string()
            } else {
                "podman".to_string()
            }),
        });
    }

    let cache = cache_dir();
    let cache_check = match std::fs::create_dir_all(&cache)
        .and_then(|()| std::fs::write(cache.join(".doctor"), b"ok"))
        .and_then(|()| std::fs::remove_file(cache.join(".doctor")))
    {
        Ok(()) => Check {
            name: "cache",
            status: "ok",
            version: None,
            detail: Some(cache.display().to_string()),
        },
        Err(error) => Check {
            name: "cache",
            status: "fail",
            version: None,
            detail: Some(format!("{}: {error}", cache.display())),
        },
    };
    checks.push(cache_check);

    for kind in rpmx_core::runner::RunnerAssetKind::ALL {
        let count = count_cached_assets(&cache, kind.cache_name());
        checks.push(if count > 0 {
            Check {
                name: kind.name(),
                status: "ok",
                version: None,
                detail: Some(format!("{count} cached")),
            }
        } else {
            Check {
                name: kind.name(),
                status: "warn",
                version: None,
                detail: Some("none cached; the first build that needs it resolves one".to_string()),
            }
        });
    }

    if let Some(recipe) = &args.recipe {
        checks.extend(recipe_checks(recipe)?);
    }

    let failed = missing_core || checks.iter().any(|check| check.status == "fail");

    if rpmx_core::output::json() {
        let value = serde_json::json!({
            "schema": 1,
            "ok": !failed,
            "command": "doctor",
            "checks": checks,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|error| rpmx_core::Error::new(
                rpmx_core::error::codes::CLI_006,
                error.to_string(),
            ))?
        );
    } else {
        for check in &checks {
            let mark = match check.status {
                "ok" => "ok  ",
                "warn" => "warn",
                _ => "fail",
            };
            println!(
                "{mark}  {:<16} {:<20} {}",
                check.name,
                check.version.clone().unwrap_or_else(|| "-".to_string()),
                check.detail.clone().unwrap_or_default()
            );
        }
    }

    if args.strict && failed {
        return Err(rpmx_core::Error::new(
            rpmx_core::error::codes::CLI_006,
            "one or more required host features are missing",
        )
        .reported());
    }
    Ok(())
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn version(name: &str) -> Option<String> {
    // ssh-keygen has no non-destructive version flag.
    if name == "ssh-keygen" {
        return None;
    }
    let flag = match name {
        "ssh" | "mke2fs" => "-V",
        _ => "--version",
    };
    let output = Command::new(name).arg(flag).output().ok()?;
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).to_string()
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    text.lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().chars().take(60).collect())
}

/// Count completed runner-asset cache entries. A missing directory is zero,
/// not an error: a host that never runs that runner has no such assets.
fn count_cached_assets(cache: &std::path::Path, directory: &str) -> usize {
    let Ok(entries) = std::fs::read_dir(cache.join(directory)) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            path.join(".complete").is_file() && path.join("root").is_dir()
        })
        .count()
}

fn cache_dir() -> PathBuf {
    if let Ok(base) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(base).join("rpmx");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache").join("rpmx");
    }
    PathBuf::from(".rpmx-cache")
}

/// Report whether the roots a recipe needs are already cached.
///
/// This is the offline-readiness question a cross build asks: both the host
/// build root and the target sysroot must be assembled before `--offline`
/// can work.
fn recipe_checks(recipe: &std::path::Path) -> Result<Vec<Check>, rpmx_core::Error> {
    use crate::commands::autoroot;

    let bundle = rpmx_core::load_recipe_bundle(recipe)?;
    let arch = autoroot::declared_arch(&bundle.recipe, &bundle.root, None)?
        .unwrap_or_else(|| rpmx_core::arch::native_arch().to_string());
    let policy = crate::commands::PlanPolicy {
        dist_tag: None,
        build_release: None,
    };
    let (_, plan) = crate::commands::load_plan(recipe, &arch, None, false, true, policy)?;
    let Some(requirement) =
        autoroot::requirement(&bundle.recipe, &plan, &bundle.root, None, &arch)?
    else {
        return Ok(vec![Check {
            name: "build root",
            status: "warn",
            version: None,
            detail: Some("the recipe declares no build.root".to_string()),
        }]);
    };

    let mut checks = Vec::new();
    let cached = autoroot::cached_lock_for_requirement(&requirement.digest).is_some();
    checks.push(Check {
        name: "build root",
        status: if cached { "ok" } else { "warn" },
        version: None,
        detail: Some(format!(
            "{} · {} · {}",
            requirement.name,
            requirement
                .profile
                .archs
                .first()
                .map(String::as_str)
                .unwrap_or("?"),
            if cached {
                "cached"
            } else {
                "not cached; the first build resolves it"
            }
        )),
    });
    if let Some(sysroot) = &requirement.sysroot {
        let cached = autoroot::cached_lock_for_requirement(&sysroot.digest).is_some();
        checks.push(Check {
            name: "cross sysroot",
            status: if cached { "ok" } else { "warn" },
            version: None,
            detail: Some(format!(
                "{} · {arch} · {}",
                sysroot.name,
                if cached {
                    "cached"
                } else {
                    "not cached; the first build assembles it"
                }
            )),
        });
    }
    Ok(checks)
}

#[cfg(test)]
mod tests {
    use super::count_cached_assets;

    #[test]
    fn only_completed_assets_are_counted() {
        let cache = tempfile::tempdir().expect("cache");
        let assets = cache.path().join("appliances");
        std::fs::create_dir_all(&assets).expect("assets");
        let complete = assets.join("complete");
        std::fs::create_dir_all(complete.join("root")).expect("root");
        std::fs::write(complete.join(".complete"), "sha512:aa").expect("marker");
        let partial = assets.join("partial");
        std::fs::create_dir_all(partial.join("root")).expect("partial root");
        assert_eq!(count_cached_assets(cache.path(), "appliances"), 1);
    }

    #[test]
    fn a_missing_asset_directory_is_zero() {
        let cache = tempfile::tempdir().expect("cache");
        assert_eq!(count_cached_assets(cache.path(), "appliances"), 0);
    }
}
