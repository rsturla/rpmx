use super::{BwrapBackend, PodmanBackend, PodmanSource};
use crate::runner::{ExecutionPolicy, RunnerAssets};
use crate::runner::{Runner, RunnerConfig, RunnerKind, RunnerRequest};
use rpmx_base::Arch;
use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::Path;
use std::process::Command;

fn executable(root: &Path, command: &str) {
    let dependencies = Command::new("ldd").arg(command).output().unwrap();
    assert!(dependencies.status.success(), "ldd {command}");
    let libraries = String::from_utf8(dependencies.stdout).unwrap();
    for source in std::iter::once(command).chain(
        libraries
            .split_whitespace()
            .filter(|word| word.starts_with('/')),
    ) {
        let target = root.join(source.trim_start_matches('/'));
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(source, target).unwrap();
    }
}

#[test]
#[ignore = "requires rootless Podman plus host shadow-utils, coreutils and util-linux"]
fn podman_transactions_can_manage_accounts_and_file_ownership() {
    account_transaction(RunnerKind::Podman);
}

#[test]
#[ignore = "requires delegated UID maps, bubblewrap and host shadow-utils, coreutils and util-linux"]
fn native_transactions_can_manage_accounts_and_file_ownership() {
    account_transaction(RunnerKind::Bwrap);
}

fn account_transaction(kind: RunnerKind) {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    // This is an execution fixture, not a repository closure. Actual
    // package solving and RPM lifecycle tests live at the application seam.
    for command in [
        "/usr/bin/bash",
        "/usr/bin/sh",
        "/usr/bin/sleep",
        "/usr/bin/id",
        "/usr/bin/cat",
        "/usr/bin/stat",
        "/usr/bin/mkdir",
        "/usr/bin/chown",
        "/usr/bin/chmod",
        "/usr/bin/groupadd",
        "/usr/bin/groupdel",
        "/usr/bin/useradd",
        "/usr/bin/userdel",
        "/usr/bin/setpriv",
        "/usr/bin/chroot",
    ] {
        executable(&root, command);
    }
    std::os::unix::fs::symlink("usr/bin", root.join("bin")).unwrap();
    for path in [
        "tmp",
        "dev",
        "proc",
        "sys",
        "etc",
        "etc/default",
        "var/lib",
        "var/log",
        "home",
    ] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    for (name, contents) in [
        ("passwd", "root:x:0:0:root:/root:/bin/sh\n"),
        ("group", "root:x:0:\n"),
        ("shadow", "root:!:1:0:99999:7:::\n"),
        ("gshadow", "root:!::\n"),
        (
            "nsswitch.conf",
            "passwd: files\ngroup: files\nshadow: files\n",
        ),
        (
            "login.defs",
            "UID_MIN 1000\nUID_MAX 60000\nGID_MIN 1000\nGID_MAX 60000\n",
        ),
        ("default/useradd", "CREATE_MAIL_SPOOL=no\n"),
    ] {
        std::fs::write(root.join("etc").join(name), contents).unwrap();
    }
    for name in ["shadow", "gshadow"] {
        std::fs::set_permissions(
            root.join("etc").join(name),
            std::fs::Permissions::from_mode(0o0),
        )
        .unwrap();
    }
    let workspace = directory.path().join("build");
    let stage = directory.path().join("stage");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&stage).unwrap();
    let before = rpmx_store::tree::digest_root(&root).unwrap();
    let arch = Arch::current().unwrap();
    let transaction_root = directory.path().join("private-root");
    crate::test_runner::copy_workspace(&root, &transaction_root).unwrap();
    let backend: Box<dyn Runner + '_> = match kind {
        RunnerKind::Bwrap => Box::new(BwrapBackend {
            root: Some(&transaction_root),
        }),
        RunnerKind::Podman => Box::new(PodmanBackend {
            source: PodmanSource::Root(&transaction_root),
            arch,
        }),
        _ => unreachable!("this fixture selects native host backends"),
    };
    let mut session = backend
        .start(&RunnerConfig {
            policy: ExecutionPolicy::default(),
            writable_root: true,
            host: arch,
            exec: arch,
            target: arch,
            sysroot: None,
            workspace: &workspace,
            stage: &stage,
            assets: RunnerAssets::default(),
            jobs: NonZeroU32::new(1).unwrap(),
            timeout: NonZeroU64::new(30).unwrap(),
        })
        .unwrap();
    let script = r#"
set -eux
test "$(id -u)" = 0
test "$(id -g)" = 0
set +x
while read -r name value rest; do
    case "$name" in
        NoNewPrivs:) test "$value" = 1 || { echo "NoNewPrivs must be 1, found $value" >&2; exit 70; } ;;
        CapEff:) effective_capabilities=$value ;;
        CapPrm:) permitted_capabilities=$value ;;
        CapBnd:) bounding_capabilities=$value ;;
    esac
done </proc/self/status
set -x
# These are real account and ownership operations used by RPM scriptlets.
groupadd -g 23456 rpmx-probe
useradd -u 23456 -g 23456 -M -d /nonexistent -s /bin/sh rpmx-probe
mkdir -p /var/lib/rpmx-probe
printf 'owned payload\n' >/var/lib/rpmx-probe/data
chown 23456:23456 /var/lib/rpmx-probe/data
test "$effective_capabilities" = 00000000000400db
test "$permitted_capabilities" = 00000000000400db
test "$bounding_capabilities" = 00000000000400db
chmod 640 /var/lib/rpmx-probe/data
test "$(stat -c '%u:%g:%a' /var/lib/rpmx-probe/data)" = 23456:23456:640
setpriv --reuid=23456 --regid=23456 --clear-groups cat /var/lib/rpmx-probe/data >ownership-result
test "$(cat ownership-result)" = 'owned payload'
test "$(chroot / /bin/sh -c 'id -u')" = 0
userdel rpmx-probe
groupdel rpmx-probe
if id rpmx-probe; then echo 'account survived cleanup' >&2; exit 71; fi
printf 'private-transaction-ok\n' >result
"#;
    let args = vec!["-c".into(), script.into()];
    let env = BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]);
    let result = session.run(&RunnerRequest {
        timeout: NonZeroU64::new(30).unwrap(),
        command: "/bin/bash",
        args: &args,
        cwd: rpmx_base::guest::BUILD,
        env: &env,
        label: "private account transaction",
        logs: None,
    });
    let finished = session.finish();
    assert!(result.is_ok(), "{result:?}; cleanup: {finished:?}");
    finished.unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("result")).unwrap(),
        "private-transaction-ok\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("ownership-result")).unwrap(),
        "owned payload\n"
    );
    assert_eq!(
        rpmx_store::tree::digest_root(&root).unwrap(),
        before,
        "transaction writes escaped into the original root"
    );
}
