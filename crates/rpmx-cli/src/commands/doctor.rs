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

    for (name, required) in [
        ("bwrap", false),
        ("podman", false),
        ("dnf5", false),
        ("tar", true),
        ("git", false),
        ("ssh", false),
        ("mke2fs", false),
        ("debugfs", false),
        ("unshare", false),
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

    let qemu = ["qemu-system-x86_64", "qemu-system-aarch64"]
        .into_iter()
        .find(|name| on_path(name).is_some());
    checks.push(match qemu {
        Some(name) => Check {
            name: "qemu-system",
            status: "ok",
            version: version(name),
            detail: Some(name.to_string()),
        },
        None => Check {
            name: "qemu-system",
            status: "warn",
            version: None,
            detail: Some("foreign-architecture builds only".to_string()),
        },
    });

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

fn cache_dir() -> PathBuf {
    if let Ok(base) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(base).join("rpmx");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache").join("rpmx");
    }
    PathBuf::from(".rpmx-cache")
}
