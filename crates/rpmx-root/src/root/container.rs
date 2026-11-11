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
    image_digest_using(Path::new("podman"), image, arch)
}

fn image_digest_using(podman: &Path, image: &str, arch: &str) -> Result<String> {
    let target = rpmx_base::arch::Arch::parse(arch)?;
    let scratch = tempfile::Builder::new()
        .prefix("rpmx-builder-")
        .tempdir()
        .map_err(|error| {
            Error::new(
                codes::ROOT_002,
                format!("cannot name builder manifest: {error}"),
            )
        })?;
    let name = format!(
        "localhost/rpmx-builder-{}:manifest",
        scratch
            .path()
            .file_name()
            .unwrap()
            .as_encoded_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    // Podman's image .Digest may describe the parent index rather than the
    // selected image. A temporary index derives descriptors from actual
    // manifest bytes, for both OCI and Docker images, without fetching layers.
    // Create first: a failed add can leave a partial index which needs cleanup.
    manifest_command(podman, &["manifest", "create", &name])?;
    let _manifest = TemporaryManifest {
        podman,
        name: &name,
    };
    manifest_command(podman, &["manifest", "add", "--all", &name, image])?;
    let output = manifest_command(podman, &["manifest", "inspect", &name])?;
    platform_digest(&output, image, target.oci_arch())
}

struct TemporaryManifest<'a> {
    podman: &'a Path,
    name: &'a str,
}

impl Drop for TemporaryManifest<'_> {
    fn drop(&mut self) {
        let _ = Command::new(self.podman)
            .args(["manifest", "rm", self.name])
            .output();
    }
}

fn manifest_command(podman: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new(podman)
        .args(arguments)
        .output()
        .map_err(|error| {
            Error::new(
                codes::ROOT_001,
                format!("cannot run podman manifest: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::ROOT_002,
            format!(
                "podman manifest {} failed: {}",
                arguments[1],
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(output.stdout)
}

fn platform_digest(bytes: &[u8], image: &str, architecture: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        Error::new(
            codes::ROOT_002,
            format!("invalid builder manifest: {error}"),
        )
    })?;
    let manifests = value
        .get("manifests")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::new(codes::ROOT_002, "builder manifest has no image descriptors"))?;
    let mut matches = manifests.iter().filter(|manifest| {
        manifest
            .pointer("/platform/architecture")
            .and_then(serde_json::Value::as_str)
            == Some(architecture)
            && manifest
                .pointer("/platform/os")
                .and_then(serde_json::Value::as_str)
                == Some("linux")
    });
    let selected = matches.next().ok_or_else(|| Error::new(codes::ROOT_002,
        format!("builder image {image} has no linux/{architecture} manifest; a {architecture} image is required")))?;
    if matches.next().is_some() {
        return Err(Error::new(codes::ROOT_002,
            format!("builder image {image} has multiple linux/{architecture} manifests; select a platform manifest explicitly")));
    }
    selected
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .filter(|digest| !digest.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Error::new(codes::ROOT_002, "builder image descriptor has no digest"))
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

#[cfg(test)]
fn ensure_image(image: &str) -> Result<String> {
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

#[cfg(test)]
fn inspect_image(image: &str, format: &str) -> Result<String> {
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
    fn platform_descriptors_select_linux_architecture_and_reject_ambiguity() {
        let index = serde_json::json!({"manifests": [
            {"digest": "sha256:arm", "platform": {"os": "linux", "architecture": "arm64"}},
            {"digest": "sha256:windows", "platform": {"os": "windows", "architecture": "amd64"}},
            {"digest": "sha256:x86", "platform": {"os": "linux", "architecture": "amd64"}}
        ]});
        let bytes = serde_json::to_vec(&index).unwrap();
        assert_eq!(
            super::platform_digest(&bytes, "image", "amd64").unwrap(),
            "sha256:x86"
        );
        assert_eq!(
            super::platform_digest(&bytes, "image", "arm64").unwrap(),
            "sha256:arm"
        );
        assert!(super::platform_digest(&bytes, "image", "riscv64")
            .unwrap_err()
            .to_string()
            .contains("no linux/riscv64"));
        let mut ambiguous = index.clone();
        ambiguous["manifests"]
            .as_array_mut()
            .unwrap()
            .push(index["manifests"][0].clone());
        assert!(
            super::platform_digest(&serde_json::to_vec(&ambiguous).unwrap(), "image", "arm64")
                .unwrap_err()
                .to_string()
                .contains("multiple linux/arm64")
        );
        for invalid in [
            b"{}".as_slice(),
            b"invalid",
            br#"{"manifests":[{"platform":{"os":"linux","architecture":"amd64"}}]}"#,
        ] {
            assert!(super::platform_digest(invalid, "image", "amd64").is_err());
        }
    }

    #[test]
    fn canonical_manifest_metadata_replaces_ambiguous_image_inspection_and_is_cleaned_up() {
        use std::os::unix::fs::PermissionsExt;
        let work = tempfile::tempdir().unwrap();
        let podman = work.path().join("podman");
        std::fs::write(
            &podman,
            r##"#!/bin/sh
set -eu
cd "$(dirname "$0")"
printf '%s\n' "$*" >> calls
case "$1 $2" in
  'manifest create') touch owned ;;
  'manifest add')
    test "$3" = --all
    test ! -f fail-add || { echo 'cannot read pinned manifest' >&2; exit 1; }
    ;;
  'manifest inspect')
    case "$3" in
      localhost/rpmx-builder-*) cat response ;;
      *) echo 'source inspection unavailable' >&2; exit 1 ;;
    esac
    ;;
  'image inspect')
    # Podman 5.7 can pair the index digest with a selected image's arch.
    case "$4" in
      '{{.Digest}}') echo sha256:index ;;
      '{{.Architecture}}') echo amd64 ;;
      *) exit 1 ;;
    esac
    ;;
  'manifest rm') rm owned ;;
  *) echo 'ambiguous image inspection must never be used' >&2; exit 1 ;;
esac
"##,
        )
        .unwrap();
        std::fs::set_permissions(&podman, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Reproduce the old fallback's inputs: source inspection fails, yet
        // image inspection supplies an index digest paired with a native arch.
        assert!(!std::process::Command::new(&podman)
            .args(["manifest", "inspect", "registry/image@sha256:index"])
            .output()
            .unwrap()
            .status
            .success());
        for (field, expected) in [
            ("{{.Digest}}", "sha256:index"),
            ("{{.Architecture}}", "amd64"),
        ] {
            let output = std::process::Command::new(&podman)
                .args([
                    "image",
                    "inspect",
                    "--format",
                    field,
                    "registry/image@sha256:index",
                ])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), expected);
        }
        let response = work.path().join("response");
        std::fs::write(&response, br#"{"manifests":[{"digest":"sha256:platform","platform":{"os":"linux","architecture":"amd64"}}]}"#).unwrap();
        assert_eq!(
            super::image_digest_using(&podman, "registry/image@sha256:index", "x86_64").unwrap(),
            "sha256:platform"
        );
        assert!(!work.path().join("owned").exists());
        let calls = std::fs::read_to_string(work.path().join("calls")).unwrap();
        assert!(calls
            .lines()
            .any(|line| line.starts_with("manifest add --all ")
                && line.ends_with("registry/image@sha256:index")));
        assert!(calls.lines().any(|line| line.starts_with("manifest rm ")));

        // Network/add errors and malformed metadata must also remove the
        // empty or partially populated temporary list, without accepting an
        // image-level digest as a substitute.
        std::fs::write(work.path().join("fail-add"), b"").unwrap();
        let error =
            super::image_digest_using(&podman, "missing@sha256:pinned", "x86_64").unwrap_err();
        assert!(error.to_string().contains("cannot read pinned manifest"));
        assert!(!work.path().join("owned").exists());
        std::fs::remove_file(work.path().join("fail-add")).unwrap();
        std::fs::write(&response, b"invalid metadata").unwrap();
        assert!(super::image_digest_using(&podman, "image", "x86_64").is_err());
        assert!(!work.path().join("owned").exists());
    }

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
    #[ignore = "requires Podman and access to the Fedora image registry"]
    fn an_oci_index_and_its_platform_manifests_resolve_to_the_same_locked_bytes() {
        let image = "registry.fedoraproject.org/fedora@sha256:f08613fffc95f5c85c019b2f4ac9e9fb9bb84b9cfd59275671b84169567b157e";
        for (arch, expected) in [
            (
                "x86_64",
                "sha256:6dd98f73357d96fc61501c35364953a159477d3ff1e944e88aae933c8d2d76b5",
            ),
            (
                "aarch64",
                "sha256:3ea357b85db3770aeef583a4974c45032d6ad2467dc47587481b509145a6caf6",
            ),
        ] {
            assert_eq!(super::image_digest_for_arch(image, arch).unwrap(), expected);
            let pinned = super::locked_image_reference(image, expected);
            assert_eq!(
                super::image_digest_for_arch(&pinned, arch).unwrap(),
                expected
            );
            assert_eq!(
                super::ensure_locked_image(image, expected, arch).unwrap(),
                pinned
            );
        }
        // A cached native image must not hide another platform behind the
        // same remote tag. This is how cross-architecture root locks resolve.
        let tag = "registry.fedoraproject.org/fedora:44";
        super::ensure_image(tag).unwrap();
        let remote = super::manifest_command(
            std::path::Path::new("podman"),
            &["manifest", "inspect", tag],
        )
        .unwrap();
        for (arch, oci_arch) in [("x86_64", "amd64"), ("aarch64", "arm64")] {
            let expected = super::platform_digest(&remote, tag, oci_arch).unwrap();
            assert_eq!(super::image_digest_for_arch(tag, arch).unwrap(), expected);
        }
        // An index pin is not a platform pin, even when the current machine
        // happens to be a supported architecture.
        assert!(
            super::ensure_locked_image(image, image.split_once('@').unwrap().1, "x86_64").is_err()
        );
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
