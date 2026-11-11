use std::path::{Path, PathBuf};
use std::process::Command;

use rpmx_base::error::{codes, Error, Result};

pub(crate) fn podman_run(
    image: &str,
    work: &Path,
    local_repositories: &[(PathBuf, String)],
    args: &[String],
) -> Result<std::process::Output> {
    let mut process = Command::new("podman");
    process
        .arg("run")
        .arg("--rm")
        .arg("--pull=missing")
        .arg("--arch")
        .arg(rpmx_base::arch::Arch::current()?.to_string())
        .arg("-v")
        .arg(format!("{}:/work:Z", work.display()))
        .arg("-w")
        .arg("/work");
    for (source, target) in local_repositories {
        process
            .arg("-v")
            .arg(format!("{}:{target}:ro,Z", source.display()));
    }
    for variable in [
        "http_proxy",
        "https_proxy",
        "no_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ] {
        if let Ok(value) = std::env::var(variable) {
            process.arg("-e").arg(format!("{variable}={value}"));
        }
    }
    process.arg(image).args(args);
    process.output().map_err(|error| {
        Error::new(
            codes::ROOT_001,
            format!("cannot run the resolver container: {error}"),
        )
    })
}

pub(crate) fn image_digest_for_arch(image: &str, arch: &str) -> Result<String> {
    let target = rpmx_base::arch::Arch::parse(arch)?;
    let oci_arch = target.oci_arch();
    if let Ok(output) = Command::new("podman")
        .args(["manifest", "inspect", image])
        .output()
    {
        if output.status.success() {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                if let Some(manifests) = value.get("manifests").and_then(|value| value.as_array()) {
                    for manifest in manifests {
                        let platform = manifest.get("platform");
                        let architecture = platform
                            .and_then(|platform| platform.get("architecture"))
                            .and_then(|value| value.as_str());
                        let os = platform
                            .and_then(|platform| platform.get("os"))
                            .and_then(|value| value.as_str());
                        if architecture == Some(oci_arch) && os == Some("linux") {
                            if let Some(digest) =
                                manifest.get("digest").and_then(|value| value.as_str())
                            {
                                return Ok(digest.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    // A local image has no manifest index. Accept it only when its recorded
    // architecture matches the requested one.
    let digest = ensure_image(image)?;
    let actual_arch = inspect_image(image, "{{.Architecture}}")?;
    let expected = target.oci_arch();
    if actual_arch != expected {
        return Err(Error::new(
            codes::ROOT_002,
            format!("builder image {image} is {actual_arch}, but a {expected} image is required"),
        ));
    }
    Ok(digest)
}

/// Address the locked platform manifest, even when the original tag has moved.
pub(crate) fn locked_image_reference(image: &str, digest: &str) -> String {
    let repository = image.split('@').next().unwrap_or(image);
    // Only a colon in the final name component denotes a tag. Registry ports,
    // IPv6 addresses and the docker:// transport must remain intact.
    let component = repository.rfind('/').map_or(0, |index| index + 1);
    let repository = match repository[component..].rfind(':') {
        Some(index) => &repository[..component + index],
        None => repository,
    };
    format!("{repository}@{digest}")
}

pub(crate) fn ensure_locked_image(image: &str, digest: &str, arch: &str) -> Result<String> {
    let pinned = locked_image_reference(image, digest);
    let actual = image_digest_for_arch(&pinned, arch)?;
    if actual != digest {
        return Err(Error::new(
            codes::ROOT_002,
            format!("locked builder image '{pinned}' has digest {actual}, expected {digest}"),
        ));
    }
    Ok(pinned)
}

pub(crate) fn ensure_image(image: &str) -> Result<String> {
    match inspect_image(image, "{{.Digest}}") {
        Ok(digest) if !digest.is_empty() => Ok(digest),
        _ => {
            let status = Command::new("podman")
                .args(["pull", "--quiet", image])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|error| {
                    Error::new(codes::ROOT_001, format!("cannot run podman: {error}"))
                })?;
            if !status.success() {
                return Err(Error::new(
                    codes::ROOT_002,
                    format!("cannot pull builder image '{image}'"),
                ));
            }
            let digest = inspect_image(image, "{{.Digest}}")?;
            if digest.is_empty() {
                return Err(Error::new(
                    codes::ROOT_002,
                    format!("builder image '{image}' has no digest"),
                ));
            }
            Ok(digest)
        }
    }
}

pub(crate) fn inspect_image(image: &str, format: &str) -> Result<String> {
    let output = Command::new("podman")
        .args(["image", "inspect", "--format", format, image])
        .output()
        .map_err(|error| {
            Error::new(
                codes::ROOT_001,
                format!("cannot inspect image '{image}': {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::ROOT_002,
            format!(
                "cannot inspect image '{image}': {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn podman_run_assembly(
    image: &str,
    work: &Path,
    root: &Path,
    target: rpmx_base::arch::Arch,
    args: &[String],
) -> Result<std::process::Output> {
    let mut process = Command::new("podman");
    process
        .arg("run")
        .arg("--rm")
        .arg("--pull=missing")
        .arg("--arch")
        .arg(target.to_string());
    process
        .arg("--network=none")
        .arg("--read-only")
        .arg("--tmpfs")
        .arg("/tmp:rw,size=512m")
        .arg("--pids-limit=1024")
        .arg("--cap-drop=all")
        .arg("--cap-add=CHOWN,DAC_OVERRIDE,FOWNER,FSETID,SETGID,SETUID,SYS_CHROOT,SYS_ADMIN,MKNOD")
        .arg("--security-opt=no-new-privileges")
        .arg("--security-opt=seccomp=unconfined")
        .arg("-v")
        .arg(format!("{}:/work:Z", work.display()))
        .arg("-v")
        .arg(format!("{}:/root:Z", root.display()))
        .arg("-w")
        .arg("/work");
    process.arg(image).args(args);
    process.output().map_err(|error| {
        Error::new(
            codes::ROOT_012,
            format!("cannot run the assembly container: {error}"),
        )
    })
}

pub enum OfflineRootMode<'a> {
    Install { rpms: &'a [PathBuf] },
    Query,
}

pub struct OfflineRootCommand<'a> {
    pub root: &'a Path,
    pub image: &'a str,
    pub target: rpmx_base::arch::Arch,
    /// Skip dependency and conflict checks when installing.
    pub nodeps: bool,
    pub mode: OfflineRootMode<'a>,
}

/// Run one offline RPM operation against an assembled root.
pub fn run_offline_root(command: OfflineRootCommand<'_>) -> Result<std::process::Output> {
    let mut process = Command::new("podman");
    process
        .arg("run")
        .arg("--rm")
        .arg("--pull=missing")
        .arg("--arch")
        .arg(command.target.to_string())
        .arg("--network=none")
        .arg("--read-only")
        .arg("--cap-drop=all")
        .arg("--security-opt=no-new-privileges")
        .arg("-v")
        .arg(format!("{}:/root:Z", command.root.display()));
    match &command.mode {
        OfflineRootMode::Install { rpms } => {
            let nodeps = if command.nodeps { " --nodeps" } else { "" };
            process.arg("--cap-add=CHOWN,DAC_OVERRIDE,FOWNER,FSETID,SETGID,SETUID,SYS_CHROOT");
            for rpm in *rpms {
                let name = rpm.file_name().ok_or_else(|| {
                    Error::new(
                        codes::ROOT_012,
                        format!("cannot mount {}: it has no file name", rpm.display()),
                    )
                })?;
                process.arg("-v").arg(format!(
                    "{}:/rpms/{}:ro,Z",
                    rpm.display(),
                    name.to_string_lossy()
                ));
            }
            process.arg(command.image).args([
                "sh",
                "-c",
                &format!(
                    "rpm --root /root -i --replacefiles --replacepkgs \
                     --oldpackage --noscripts{nodeps} /rpms/*.rpm"
                ),
            ]);
        }
        OfflineRootMode::Query => {
            process.arg(command.image).args([
                "rpm",
                "--root",
                "/root",
                "-qa",
                "--qf",
                "%{NAME}|%{EPOCH}|%{VERSION}|%{RELEASE}|%{ARCH}\n",
            ]);
        }
    }
    process.output().map_err(|error| {
        Error::new(
            codes::ROOT_012,
            format!("cannot run the offline root container: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn locked_references_replace_tags_without_damaging_repository_names() {
        for (image, repository) in [
            ("fedora:44", "fedora"),
            (
                "registry.example:5000/team/fedora:44",
                "registry.example:5000/team/fedora",
            ),
            ("[::1]:5000/team/fedora", "[::1]:5000/team/fedora"),
            (
                "docker://registry.example:5000/fedora:44",
                "docker://registry.example:5000/fedora",
            ),
            (
                "registry.example/fedora@sha256:old",
                "registry.example/fedora",
            ),
            (
                "registry.example/fedora:44@sha256:old",
                "registry.example/fedora",
            ),
        ] {
            assert_eq!(
                super::locked_image_reference(image, "sha256:locked"),
                format!("{repository}@sha256:locked"),
                "{image}"
            );
        }
    }

    #[test]
    #[ignore = "requires rootless Podman and access to the Fedora image registry"]
    fn a_locked_platform_image_runs_when_its_original_tag_no_longer_exists() {
        let arch = rpmx_base::arch::Arch::current().unwrap();
        let image = "registry.fedoraproject.org/fedora:44";
        let digest = super::image_digest_for_arch(image, &arch.to_string()).unwrap();
        // A missing tag is a stronger control than a moved tag: assembly must
        // obtain its bytes entirely from the recorded platform digest.
        let missing_tag = "registry.fedoraproject.org/fedora:rpmx-deliberately-absent";
        let pinned = super::ensure_locked_image(missing_tag, &digest, &arch.to_string()).unwrap();
        let work = tempfile::tempdir().unwrap();
        let result = super::podman_run(
            &pinned,
            work.path(),
            &[],
            &[
                "sh".to_string(),
                "-c".to_string(),
                "printf locked-builder".to_string(),
            ],
        )
        .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, b"locked-builder");
        let other_arch = if arch.to_string() == "x86_64" {
            "aarch64"
        } else {
            "x86_64"
        };
        let wrong_arch = super::ensure_locked_image(missing_tag, &digest, other_arch).unwrap_err();
        assert!(
            wrong_arch.to_string().contains("image is required"),
            "{wrong_arch}"
        );
        let missing_digest = format!("sha256:{}", "0".repeat(64));
        assert!(
            super::ensure_locked_image(missing_tag, &missing_digest, &arch.to_string()).is_err()
        );
    }

}
