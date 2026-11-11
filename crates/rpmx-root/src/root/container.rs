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
        // Resolvers can read the same repository concurrently. A private
        // SELinux label would revoke another resolver's access.
        process
            .arg("-v")
            .arg(format!("{}:{target}:ro,z", source.display()));
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
                    "{}:/rpms/{}:ro,z",
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
    use super::podman_run;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "requires Podman and an SELinux enforcing host"]
    fn concurrent_resolvers_can_read_the_same_local_repository() {
        assert_eq!(
            std::fs::read_to_string("/sys/fs/selinux/enforce")
                .unwrap()
                .trim(),
            "1",
            "this regression requires SELinux enforcement"
        );
        let image = std::env::var("RPMX_TEST_IMAGE")
            .unwrap_or_else(|_| "registry.fedoraproject.org/fedora:44".to_string());
        super::ensure_image(&image).unwrap();
        let repository = tempfile::tempdir().unwrap();
        std::fs::write(repository.path().join("value"), b"shared repository\n").unwrap();
        let first_work = tempfile::tempdir().unwrap();
        let second_work = tempfile::tempdir().unwrap();
        let first_path = first_work.path().to_path_buf();
        let first_repo = repository.path().to_path_buf();
        let first_image = image.clone();
        let first = std::thread::spawn(move || {
            podman_run(
                &first_image,
                &first_path,
                &[(first_repo, "/repository".to_string())],
                &[
                    "sh".to_string(),
                    "-ec".to_string(),
                    "touch /work/ready; for i in $(seq 1 300); do \
                     if test -e /work/release; then exec cat /repository/value; fi; \
                     sleep 0.1; done; exit 2"
                        .to_string(),
                ],
            )
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        while !first_work.path().join("ready").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let ready = first_work.path().join("ready").exists();
        let second = if ready {
            Some(podman_run(
                &image,
                second_work.path(),
                &[(repository.path().to_path_buf(), "/repository".to_string())],
                &["cat".to_string(), "/repository/value".to_string()],
            ))
        } else {
            None
        };
        std::fs::write(first_work.path().join("release"), b"").unwrap();
        let first = first.join().unwrap().unwrap();
        assert!(ready, "first resolver did not become ready: {first:?}");
        let second = second.unwrap().unwrap();
        assert!(second.status.success(), "second resolver: {second:?}");
        assert!(first.status.success(), "first resolver: {first:?}");
        assert_eq!(first.stdout, b"shared repository\n");
        assert_eq!(second.stdout, b"shared repository\n");
    }
}
