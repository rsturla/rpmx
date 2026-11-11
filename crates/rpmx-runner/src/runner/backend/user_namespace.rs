//! A full subordinate-ID namespace for a bubblewrap command.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rpmx_base::error::{codes, Error, Result};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const UNPRIVILEGED_ID: u32 = 65534;
const REQUIREMENTS: &str = "bubblewrap requires util-linux unshare with --map-auto, newuidmap, newgidmap, and at least 65536 subordinate IDs in both /etc/subuid and /etc/subgid; run rpmx as an unprivileged user and configure the uidmap/shadow-utils allocation";

pub(super) struct UserNamespace {
    namespace: File,
    _helper: NamespaceHelper,
    pub(super) uid: u32,
    pub(super) gid: u32,
}

// Keep the helper and control socket alive together. The helper runs in its own
// process group so setup failures and session drops also reap mapping helpers.
struct NamespaceHelper {
    child: Child,
    control: UnixStream,
    errors: File,
}

impl Drop for NamespaceHelper {
    fn drop(&mut self) {
        let _ = self.control.shutdown(std::net::Shutdown::Both);
        // The child has not been waited on, so its PID cannot have been reused.
        // Kill its group before reaping, including any unfinished map helpers.
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

impl NamespaceHelper {
    fn wait_ready(
        child: Child,
        control: UnixStream,
        errors: File,
        timeout: Duration,
    ) -> Result<Self> {
        let mut helper = Self {
            child,
            control,
            errors,
        };
        let deadline = Instant::now() + timeout;
        let mut ready = [0];
        let readiness = loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "namespace setup deadline expired",
                ));
            };
            helper
                .control
                .set_read_timeout(Some(remaining))
                .map_err(setup_error)?;
            match helper.control.read(&mut ready) {
                Ok(1) => break Ok(()),
                Ok(_) => {
                    break Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "namespace helper exited before readiness",
                    ))
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => break Err(error),
            }
        };
        if let Err(error) = readiness {
            // Stop writers before collecting their diagnostic. Do not wait for
            // an unresponsive mapper merely to print why setup timed out.
            unsafe {
                libc::kill(-(helper.child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = helper.errors.seek(SeekFrom::Start(0));
            let mut details = String::new();
            let _ = Read::by_ref(&mut helper.errors)
                .take(64 * 1024)
                .read_to_string(&mut details);
            return Err(setup_error(format!(
                "namespace setup did not become ready within {timeout:?}: {error}; {}",
                details.trim()
            )));
        }
        if ready != *b"R" {
            return Err(setup_error("invalid namespace helper readiness response"));
        }
        Ok(helper)
    }
}

fn setup_error(error: impl std::fmt::Display) -> Error {
    Error::new(codes::SANDBOX_001, format!("{REQUIREMENTS}: {error}"))
}

fn validate_map(contents: &str, inner_caller: u32, outer_caller: u32) -> Result<()> {
    // Check the entire normal POSIX ID range, including root and nobody. A
    // single-ID map makes an unprivileged setuid(0) fail with EINVAL, not EPERM.
    let mut ranges = Vec::new();
    for line in contents.lines() {
        let fields = line
            .split_whitespace()
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(setup_error)?;
        let [inner, outer, count] = fields.as_slice() else {
            return Err(setup_error("invalid kernel ID mapping"));
        };
        ranges.push((*inner, *outer, *count));
    }
    ranges.sort_unstable();
    let mut covered = 0_u64;
    let mut caller_matches = false;
    for (inner, outer, count) in ranges {
        if let Some(offset) = inner_caller.checked_sub(inner) {
            if offset < count && outer.checked_add(offset) == Some(outer_caller) {
                caller_matches = true;
            }
        }
        if u64::from(inner) <= covered {
            covered = covered.max(u64::from(inner) + u64::from(count));
        }
    }
    if !caller_matches || covered < 65536 {
        return Err(setup_error(
            "the resulting namespace does not map root, nobody, and the caller across IDs 0..65535",
        ));
    }
    Ok(())
}

impl UserNamespace {
    pub(super) fn new(transaction: bool) -> Result<Self> {
        // SAFETY: these getters have no preconditions and do not change IDs.
        let (host_uid, host_gid) = unsafe { (libc::getuid(), libc::getgid()) };
        if host_uid == 0 {
            return Err(setup_error("the caller is root"));
        }
        let (control, child_control) = UnixStream::pair().map_err(setup_error)?;
        let mut command = Command::new("unshare");
        command.args([
            "--user",
            "--map-auto",
            if transaction {
                "--map-root-user"
            } else {
                "--map-user=65534"
            },
            if transaction {
                "--map-group=0"
            } else {
                "--map-group=65534"
            },
            "--setuid",
            "0",
            "--setgid",
            "0",
            "/bin/sh",
            "-eu",
            "-c",
            // This sysctl belongs to the new namespace. The initial parent
            // remains outside it and hands its FD to bubblewrap. The payload
            // drops all setup capabilities before it can run any command.
            "printf '0\\n' > /proc/sys/user/max_user_namespaces; printf R; IFS= read -r done",
            "rpmx-user-namespace",
        ]);
        control_stdio(&mut command, child_control).map_err(setup_error)?;
        // Spawn first, then close our copy: EOF must reach readiness on failure.
        let errors = tempfile::tempfile().map_err(setup_error)?;
        command.stderr(Stdio::from(errors.try_clone().map_err(setup_error)?));
        command.process_group(0);
        let child = command.spawn().map_err(setup_error)?;
        // Command owns its configured stdio handles until it is dropped.
        // Close both parent copies so a failed helper produces immediate EOF.
        drop(command);
        let helper = NamespaceHelper::wait_ready(child, control, errors, STARTUP_TIMEOUT)?;
        let process = format!("/proc/{}", helper.child.id());
        let (uid, gid) = if transaction {
            (0, 0)
        } else {
            (UNPRIVILEGED_ID, UNPRIVILEGED_ID)
        };
        validate_map(
            &std::fs::read_to_string(format!("{process}/uid_map")).map_err(setup_error)?,
            uid,
            host_uid,
        )?;
        validate_map(
            &std::fs::read_to_string(format!("{process}/gid_map")).map_err(setup_error)?,
            gid,
            host_gid,
        )?;
        let namespace = File::open(format!("{process}/ns/user")).map_err(setup_error)?;
        Ok(Self {
            namespace,
            _helper: helper,
            uid,
            gid,
        })
    }

    pub(super) fn fd(&self) -> BorrowedFd<'_> {
        self.namespace.as_fd()
    }
}

fn control_stdio(command: &mut Command, peer: UnixStream) -> std::io::Result<()> {
    command
        .stdin(Stdio::from(OwnedFd::from(peer.try_clone()?)))
        .stdout(Stdio::from(OwnedFd::from(peer)));
    Ok(())
}

pub(crate) fn check_bwrap_user_namespace() -> Result<()> {
    UserNamespace::new(false).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsRawFd;

    fn mock_helper(script: &str) -> (Child, UnixStream, File) {
        let (control, peer) = UnixStream::pair().unwrap();
        let errors = tempfile::tempfile().unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script, "namespace-test"])
            .process_group(0)
            .stderr(Stdio::from(errors.try_clone().unwrap()));
        control_stdio(&mut command, peer).unwrap();
        let child = command.spawn().unwrap();
        drop(command);
        (child, control, errors)
    }

    #[test]
    fn full_maps_preserve_the_caller_and_reject_missing_or_partial_ranges() {
        let full = "1000 1000 1\n0 524288 1000\n1001 525288 64536\n";
        validate_map(full, 1000, 1000).unwrap();
        validate_map(
            "65534 1000 1\n0 524288 65534\n65535 589822 2\n",
            65534,
            1000,
        )
        .unwrap();
        validate_map("0 1000 1\n1 524288 65536\n", 0, 1000).unwrap();
        for map in [
            "1000 1000 1\n",
            "0 524288 65536\n",
            "0 524288 60000\n1000 1000 1\n",
        ] {
            assert!(validate_map(map, 1000, 1000).is_err(), "{map}");
        }
    }

    #[test]
    fn posix_shell_readiness_works_with_high_numbered_control_sockets() {
        // Concurrent sessions often allocate socket descriptors above the
        // single-digit redirection range accepted by shells such as Dash.
        let _occupied = (0..32)
            .map(|_| File::open("/dev/null").unwrap())
            .collect::<Vec<_>>();
        let (control, peer) = UnixStream::pair().unwrap();
        assert!(control.as_raw_fd() > 9 && peer.as_raw_fd() > 9);
        let errors = tempfile::tempfile().unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-eu",
                "-c",
                "printf R; IFS= read -r done; printf '%s' \"$done\"",
            ])
            .process_group(0)
            .stderr(Stdio::from(errors.try_clone().unwrap()));
        control_stdio(&mut command, peer).unwrap();
        let child = command.spawn().unwrap();
        drop(command);
        let mut helper =
            NamespaceHelper::wait_ready(child, control, errors, Duration::from_secs(2)).unwrap();
        helper.control.write_all(b"stop\n").unwrap();
        let mut response = [0; 4];
        helper.control.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"stop");
    }

    #[test]
    fn a_stalled_namespace_setup_is_bounded_and_its_process_group_is_reaped() {
        let (child, control, errors) = mock_helper("sleep 60 & wait");
        let pid = child.id();
        let started = Instant::now();
        let error = NamespaceHelper::wait_ready(child, control, errors, Duration::from_millis(50))
            .err()
            .expect("setup must time out");
        assert!(error.to_string().contains("namespace setup"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "helper must be reaped"
        );
    }

    #[test]
    fn a_failed_mapper_keeps_its_prerequisite_diagnostic() {
        let (child, control, errors) =
            mock_helper("echo 'newuidmap: no subordinate IDs' >&2; exit 7");
        let error = NamespaceHelper::wait_ready(child, control, errors, Duration::from_secs(2))
            .err()
            .expect("mapping must fail");
        let message = error.to_string();
        assert!(
            message.contains("newuidmap: no subordinate IDs"),
            "{message}"
        );
        assert!(
            message.contains("/etc/subuid") && message.contains("/etc/subgid"),
            "{message}"
        );
    }

    #[test]
    fn dropping_a_ready_namespace_helper_reaps_it() {
        let (child, control, errors) = mock_helper("printf R; sleep 60 & wait");
        let pid = child.id();
        let helper =
            NamespaceHelper::wait_ready(child, control, errors, Duration::from_secs(2)).unwrap();
        assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
        drop(helper);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
}
