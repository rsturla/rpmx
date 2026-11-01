use std::path::Path;
use std::process::Command;

use rpmx_base::error::{codes, Error, Result};

pub(crate) fn podman_run(
    image: &str,
    work: &Path,
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
