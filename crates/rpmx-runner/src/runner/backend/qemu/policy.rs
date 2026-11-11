use super::guest::BUILD_ROOT_MOUNT;
use super::ssh::shell_quote;
use rpmx_base::error::{codes, Error, Result};
use std::time::Duration;

pub(super) const PAYLOAD_SLICE: &str = "rpmx-payload.slice";

pub(super) struct PayloadPolicy {
    pub network: bool,
    pub readonly_stage: bool,
    pub writable_root: bool,
    pub mounts: Vec<(String, String)>,
}

pub(super) fn input_target(target: &str) -> Result<()> {
    if !target.starts_with("/tmp/rpmx-inputs/")
        || target.split('/').any(|part| part == ".." || part == ".")
        || !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_.-".contains(&byte))
    {
        return Err(Error::new(
            codes::RUNNER_004,
            format!("invalid read-only qemu input target '{target}'"),
        ));
    }
    Ok(())
}

impl PayloadPolicy {
    pub fn command(&self, timeout: Duration, unit: &str) -> String {
        let root = BUILD_ROOT_MOUNT;
        let mut args = vec![
            "timeout".into(),
            "--signal=TERM".into(),
            "--kill-after=2".into(),
            super::budget::seconds(timeout),
            "systemd-run".into(),
            "--quiet".into(),
            "--wait".into(),
            "--pipe".into(),
            "--collect".into(),
            format!("--unit={unit}"),
            format!("--slice={PAYLOAD_SLICE}"),
        ];
        let mut properties = vec![
            format!("RootDirectory={root}"),
            "WorkingDirectory=/tmp/build".into(),
            "NoNewPrivileges=yes".into(),
            "PrivatePIDs=yes".into(),
            "PrivateDevices=yes".into(),
            "ProtectControlGroups=yes".into(),
            "KillMode=control-group".into(),
            "SendSIGKILL=yes".into(),
            "TimeoutStopSec=2s".into(),
            format!(
                "RuntimeMaxSec={}s",
                super::budget::seconds(timeout + Duration::from_secs(2))
            ),
            format!("PrivateNetwork={}", if self.network { "no" } else { "yes" }),
            "TemporaryFileSystem=/tmp:rw,nodev,nosuid,mode=1777".into(),
            "BindPaths=/run/rpmx/workspace:/tmp/build".into(),
        ];
        if self.writable_root {
            properties.extend([
                "User=0".into(),
                "Group=0".into(),
                format!(
                    "CapabilityBoundingSet={}",
                    crate::runner::TRANSACTION_CAPABILITIES.join(" ")
                ),
            ]);
        } else {
            properties.extend([
                "User=65534".into(),
                "Group=65534".into(),
                "SupplementaryGroups=".into(),
                "CapabilityBoundingSet=".into(),
                "ProtectSystem=strict".into(),
                "ReadWritePaths=+/tmp/build".into(),
            ]);
        }
        let stage = "/run/rpmx/stage:/tmp/stage".to_string();
        let mut writable = vec!["/run/rpmx/workspace:/tmp/build".to_string()];
        let mut readonly = Vec::new();
        if self.readonly_stage {
            readonly.push(stage);
        } else {
            writable.push(stage);
        }
        readonly.extend(
            self.mounts
                .iter()
                .map(|(source, target)| format!("{source}:{target}")),
        );
        if self.network {
            readonly.push("/run/rpmx/resolv.conf:/etc/resolv.conf".to_string());
        }
        properties.retain(|property| {
            !property.starts_with("BindPaths=") && !property.starts_with("ReadWritePaths=")
        });
        properties.push(format!("BindPaths={}", writable.join(" ")));
        properties.push(format!(
            "ReadWritePaths=+/tmp/build{}",
            if self.readonly_stage {
                ""
            } else {
                " +/tmp/stage"
            }
        ));
        if !readonly.is_empty() {
            properties.push(format!("BindReadOnlyPaths={}", readonly.join(" ")));
        }
        for property in properties {
            args.push(format!("--property={property}"));
        }
        args.extend(["/bin/sh".into(), "-s".into()]);
        args.iter()
            .map(|value| shell_quote(value))
            .collect::<Vec<_>>()
            .join(" ")
    }
}
