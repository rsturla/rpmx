use std::path::Path;
use std::process::Command;

use rpmx_base::error::{codes, Error, Result};

/// Where the disposable build-root disk is mounted inside the appliance.
pub(super) const BUILD_ROOT_MOUNT: &str = "/run/rpmx/root";
/// The control channel: root, outside any chroot.
pub(super) const CONTROL_PORT: u16 = 22;
/// The build channel: chrooted into [`BUILD_ROOT_MOUNT`].
pub(super) const BUILD_PORT: u16 = 2222;
pub(super) const APPLIANCE_LABEL: &str = "rpmx-appliance";
pub(super) const BUILD_LABEL: &str = "rpmx-build";

pub(super) fn generate_ssh_key(key: &Path) -> Result<()> {
    let output = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(key)
        .output()
        .map_err(|error| {
            Error::new(codes::RUNNER_003, format!("cannot run ssh-keygen: {error}"))
        })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "ssh-keygen failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}

/// Inject the per-session SSH client key, host key and two-port sshd config
/// into the appliance disk. The build root is never touched.
pub(super) fn inject_ssh(
    appliance_root: &Path,
    appliance_disk: &Path,
    client_public_key: &Path,
    host_private_key: &Path,
    scratch: &Path,
) -> Result<()> {
    if !appliance_root.join("usr/sbin/sshd").exists() {
        return Err(Error::new(
            codes::RUNNER_004,
            format!(
                "the qemu appliance must include openssh-server (missing {})",
                appliance_root.join("usr/sbin/sshd").display()
            ),
        ));
    }
    let sshd_config = scratch.join("sshd_config");
    std::fs::write(&sshd_config, SSHD_CONFIG).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot write sshd config: {error}"),
        )
    })?;

    let _ = debugfs_write(appliance_disk, "mkdir /root/.ssh");
    // `debugfs write` refuses to replace an existing file, so remove the
    // packaged config and any stale keys first.
    let _ = debugfs_write(appliance_disk, "rm /etc/ssh/sshd_config");
    let _ = debugfs_write(appliance_disk, "rm /etc/ssh/ssh_host_ed25519_key");
    let _ = debugfs_write(appliance_disk, "rm /root/.ssh/authorized_keys");
    debugfs_write(
        appliance_disk,
        &format!("write {} /etc/ssh/sshd_config", sshd_config.display()),
    )?;
    debugfs_write(appliance_disk, "sif /etc/ssh/sshd_config mode 0100644")?;
    debugfs_write(appliance_disk, "sif /etc/ssh/sshd_config uid 0")?;
    debugfs_write(appliance_disk, "sif /etc/ssh/sshd_config gid 0")?;
    debugfs_write(
        appliance_disk,
        &format!(
            "write {} /root/.ssh/authorized_keys",
            client_public_key.display()
        ),
    )?;
    debugfs_write(
        appliance_disk,
        "sif /root/.ssh/authorized_keys mode 0100600",
    )?;
    debugfs_write(appliance_disk, "sif /root/.ssh/authorized_keys uid 0")?;
    debugfs_write(appliance_disk, "sif /root/.ssh/authorized_keys gid 0")?;
    debugfs_write(appliance_disk, "sif /root/.ssh mode 040700")?;
    debugfs_write(appliance_disk, "sif /root/.ssh uid 0")?;
    debugfs_write(appliance_disk, "sif /root/.ssh gid 0")?;
    debugfs_write(
        appliance_disk,
        &format!(
            "write {} /etc/ssh/ssh_host_ed25519_key",
            host_private_key.display()
        ),
    )?;
    debugfs_write(
        appliance_disk,
        "sif /etc/ssh/ssh_host_ed25519_key mode 0100600",
    )?;
    debugfs_write(appliance_disk, "sif /etc/ssh/ssh_host_ed25519_key uid 0")?;
    debugfs_write(appliance_disk, "sif /etc/ssh/ssh_host_ed25519_key gid 0")?;
    enable_unit(appliance_disk, "sshd.service")?;
    Ok(())
}

pub(super) fn inject_network(
    appliance_root: &Path,
    appliance_disk: &Path,
    scratch: &Path,
) -> Result<()> {
    let unit = appliance_root.join("usr/lib/systemd/system/systemd-networkd.service");
    if !unit.exists() {
        return Err(Error::new(
            codes::RUNNER_004,
            format!(
                "the qemu appliance must include systemd-networkd (missing {})",
                unit.display()
            ),
        ));
    }
    let config = scratch.join("20-rpmx.network");
    std::fs::write(&config, "[Match]\nType=ether\n\n[Network]\nDHCP=yes\n").map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot write {}: {error}", config.display()),
        )
    })?;
    let _ = debugfs_write(appliance_disk, "mkdir /etc/systemd/network");
    debugfs_write(
        appliance_disk,
        &format!(
            "write {} /etc/systemd/network/20-rpmx.network",
            config.display()
        ),
    )?;
    debugfs_write(
        appliance_disk,
        "sif /etc/systemd/network/20-rpmx.network mode 0100644",
    )?;
    debugfs_write(
        appliance_disk,
        "sif /etc/systemd/network/20-rpmx.network uid 0",
    )?;
    debugfs_write(
        appliance_disk,
        "sif /etc/systemd/network/20-rpmx.network gid 0",
    )?;
    enable_unit(appliance_disk, "systemd-networkd.service")?;
    Ok(())
}

/// Inject and enable the unit that mounts the build-root disk and its virtual
/// filesystems before sshd starts.
pub(super) fn inject_build_mount(appliance_disk: &Path, scratch: &Path) -> Result<()> {
    let unit = scratch.join("rpmx-buildroot.service");
    std::fs::write(&unit, BUILD_ROOT_UNIT).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot write {}: {error}", unit.display()),
        )
    })?;
    debugfs_write(
        appliance_disk,
        &format!(
            "write {} /etc/systemd/system/rpmx-buildroot.service",
            unit.display()
        ),
    )?;
    debugfs_write(
        appliance_disk,
        "sif /etc/systemd/system/rpmx-buildroot.service mode 0100644",
    )?;
    debugfs_write(
        appliance_disk,
        "sif /etc/systemd/system/rpmx-buildroot.service uid 0",
    )?;
    debugfs_write(
        appliance_disk,
        "sif /etc/systemd/system/rpmx-buildroot.service gid 0",
    )?;
    enable_unit(appliance_disk, "rpmx-buildroot.service")?;
    // sshd must not start until the build root is mounted. A .requires
    // directory makes the dependency explicit to systemd.
    let _ = debugfs_write(
        appliance_disk,
        "mkdir /etc/systemd/system/sshd.service.requires",
    );
    let _ = debugfs_write(
        appliance_disk,
        "rm /etc/systemd/system/sshd.service.requires/rpmx-buildroot.service",
    );
    debugfs_write(
        appliance_disk,
        "symlink /etc/systemd/system/sshd.service.requires/rpmx-buildroot.service /etc/systemd/system/rpmx-buildroot.service",
    )?;
    Ok(())
}

fn enable_unit(disk: &Path, unit: &str) -> Result<()> {
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system");
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system/multi-user.target.wants");
    let link = format!("/etc/systemd/system/multi-user.target.wants/{unit}");
    let _ = debugfs_write(disk, &format!("rm {link}"));
    debugfs_write(
        disk,
        &format!("symlink {link} /usr/lib/systemd/system/{unit}"),
    )
}

pub(super) fn debugfs_write(disk: &Path, command: &str) -> Result<()> {
    let output = Command::new("debugfs")
        .arg("-w")
        .arg("-R")
        .arg(command)
        .arg(disk)
        .output()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot run debugfs: {error}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() || stderr.contains("already exists") {
        return Err(Error::new(
            codes::RUNNER_003,
            format!("debugfs '{}' failed: {}", command, stderr.trim()),
        ));
    }
    Ok(())
}

pub(super) fn known_hosts(
    scratch: &Path,
    host_public_key: &Path,
    control_port: u16,
    build_port: u16,
) -> Result<std::path::PathBuf> {
    let public = std::fs::read_to_string(host_public_key).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot read {}: {error}", host_public_key.display()),
        )
    })?;
    let key = public
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    if key.is_empty() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!("{} is not an SSH public key", host_public_key.display()),
        ));
    }
    let path = scratch.join("known_hosts");
    let text = format!("[127.0.0.1]:{control_port} {key}\n[127.0.0.1]:{build_port} {key}\n");
    std::fs::write(&path, text).map_err(|error| {
        Error::new(
            codes::RUNNER_003,
            format!("cannot write {}: {error}", path.display()),
        )
    })?;
    Ok(path)
}

const SSHD_CONFIG: &str = "Port 22\n\
Port 2222\n\
PermitRootLogin prohibit-password\n\
PasswordAuthentication no\n\
PubkeyAuthentication yes\n\
KbdInteractiveAuthentication no\n\
UsePAM yes\n\
HostKey /etc/ssh/ssh_host_ed25519_key\n\
Subsystem sftp internal-sftp\n\
\n\
Match LocalPort 2222\n\
    ChrootDirectory /run/rpmx/root\n";

const BUILD_ROOT_UNIT: &str = "\
[Unit]\n\
Description=Mount the rpmx build root\n\
After=local-fs.target\n\
Before=sshd.service\n\
\n\
[Service]\n\
Type=oneshot\n\
RemainAfterExit=yes\n\
ExecStart=/bin/sh -c '\
set -e; \
mkdir -p /run/rpmx/root; \
mount -L rpmx-build /run/rpmx/root; \
mkdir -p /run/rpmx/root/proc /run/rpmx/root/dev /run/rpmx/root/sys \
         /run/rpmx/root/tmp/build /run/rpmx/root/tmp/stage; \
mount -t proc proc /run/rpmx/root/proc; \
mount -t tmpfs tmpfs /run/rpmx/root/dev; \
for d in null zero full random urandom tty; do \
  touch /run/rpmx/root/dev/$d; \
  mount --bind /dev/$d /run/rpmx/root/dev/$d; \
done; \
mkdir -p /run/rpmx/root/dev/pts /run/rpmx/root/dev/shm; \
mount --rbind /dev/pts /run/rpmx/root/dev/pts; \
mount --rbind /dev/shm /run/rpmx/root/dev/shm; \
mount --rbind /sys /run/rpmx/root/sys; \
mount --make-rslave /run/rpmx/root/dev/pts; \
mount --make-rslave /run/rpmx/root/dev/shm; \
mount --make-rslave /run/rpmx/root/sys'\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n";

#[cfg(test)]
mod tests {
    use super::{known_hosts, BUILD_PORT, CONTROL_PORT, SSHD_CONFIG};

    #[test]
    fn the_build_port_is_chrooted_and_the_control_port_is_not() {
        let after_match = SSHD_CONFIG
            .split("Match LocalPort 2222")
            .nth(1)
            .expect("match block");
        assert!(after_match.contains("ChrootDirectory /run/rpmx/root"));
        let before_match = SSHD_CONFIG
            .split("Match LocalPort 2222")
            .next()
            .expect("prelude");
        assert!(!before_match.contains("ChrootDirectory"));
        assert!(SSHD_CONFIG.starts_with(&format!("Port {CONTROL_PORT}\n")));
        assert!(SSHD_CONFIG.contains(&format!("\nPort {BUILD_PORT}\n")));
    }

    #[test]
    fn known_hosts_pins_both_session_ports() {
        let directory = tempfile::tempdir().expect("tempdir");
        let public = directory.path().join("host.pub");
        std::fs::write(&public, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest test\n")
            .expect("public key");
        let path = known_hosts(directory.path(), &public, 41234, 45678).expect("known hosts");
        let text = std::fs::read_to_string(path).expect("read");
        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest";
        assert_eq!(
            text,
            format!("[127.0.0.1]:41234 {key}\n[127.0.0.1]:45678 {key}\n")
        );
    }
}
