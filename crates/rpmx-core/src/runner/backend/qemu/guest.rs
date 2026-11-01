use std::path::Path;
use std::process::Command;

use crate::error::{codes, Error, Result};

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

pub(super) fn inject_ssh(
    root: &Path,
    disk: &Path,
    public_key: &Path,
    scratch: &Path,
) -> Result<()> {
    if !root.join("usr/sbin/sshd").exists() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "the qemu root must include openssh-server (missing {})",
                root.join("usr/sbin/sshd").display()
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
    let host_key = scratch.join("ssh_host_ed25519_key");
    let output = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&host_key)
        .output()
        .map_err(|error| {
            Error::new(
                codes::RUNNER_003,
                format!("cannot generate ssh host key: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "ssh host key generation failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    let _ = debugfs_write(disk, "mkdir /root/.ssh");
    debugfs_write(
        disk,
        &format!("write {} /etc/ssh/sshd_config", sshd_config.display()),
    )?;
    debugfs_write(disk, "sif /etc/ssh/sshd_config mode 0100644")?;
    debugfs_write(disk, "sif /etc/ssh/sshd_config uid 0")?;
    debugfs_write(disk, "sif /etc/ssh/sshd_config gid 0")?;
    debugfs_write(
        disk,
        &format!("write {} /root/.ssh/authorized_keys", public_key.display()),
    )?;
    debugfs_write(disk, "sif /root/.ssh/authorized_keys mode 0100600")?;
    debugfs_write(disk, "sif /root/.ssh/authorized_keys uid 0")?;
    debugfs_write(disk, "sif /root/.ssh/authorized_keys gid 0")?;
    debugfs_write(disk, "sif /root/.ssh mode 040700")?;
    debugfs_write(disk, "sif /root/.ssh uid 0")?;
    debugfs_write(disk, "sif /root/.ssh gid 0")?;
    debugfs_write(
        disk,
        &format!("write {} /etc/ssh/ssh_host_ed25519_key", host_key.display()),
    )?;
    debugfs_write(disk, "sif /etc/ssh/ssh_host_ed25519_key mode 0100600")?;
    debugfs_write(disk, "sif /etc/ssh/ssh_host_ed25519_key uid 0")?;
    debugfs_write(disk, "sif /etc/ssh/ssh_host_ed25519_key gid 0")?;
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system");
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system/multi-user.target.wants");
    let _ = debugfs_write(
        disk,
        "symlink /etc/systemd/system/multi-user.target.wants/sshd.service /usr/lib/systemd/system/sshd.service",
    );
    Ok(())
}

pub(super) fn inject_network(root: &Path, disk: &Path, scratch: &Path) -> Result<()> {
    let unit = root.join("usr/lib/systemd/system/systemd-networkd.service");
    if !unit.exists() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "the qemu root must include systemd-networkd (missing {})",
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
    let _ = debugfs_write(disk, "mkdir /etc/systemd/network");
    debugfs_write(
        disk,
        &format!(
            "write {} /etc/systemd/network/20-rpmx.network",
            config.display()
        ),
    )?;
    debugfs_write(
        disk,
        "sif /etc/systemd/network/20-rpmx.network mode 0100644",
    )?;
    debugfs_write(disk, "sif /etc/systemd/network/20-rpmx.network uid 0")?;
    debugfs_write(disk, "sif /etc/systemd/network/20-rpmx.network gid 0")?;
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system");
    let _ = debugfs_write(disk, "mkdir /etc/systemd/system/multi-user.target.wants");
    let _ = debugfs_write(
        disk,
        "symlink /etc/systemd/system/multi-user.target.wants/systemd-networkd.service /usr/lib/systemd/system/systemd-networkd.service",
    );
    Ok(())
}

pub(super) fn debugfs_write(disk: &Path, command: &str) -> Result<()> {
    let output = Command::new("debugfs")
        .arg("-w")
        .arg("-R")
        .arg(command)
        .arg(disk)
        .output()
        .map_err(|error| Error::new(codes::RUNNER_003, format!("cannot run debugfs: {error}")))?;
    if !output.status.success() {
        return Err(Error::new(
            codes::RUNNER_003,
            format!(
                "debugfs '{}' failed: {}",
                command,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}

const SSHD_CONFIG: &str = "Port 22\n\
PermitRootLogin prohibit-password\n\
PasswordAuthentication no\n\
PubkeyAuthentication yes\n\
UsePAM no\n\
HostKey /etc/ssh/ssh_host_ed25519_key\n\
Subsystem sftp internal-sftp\n";
