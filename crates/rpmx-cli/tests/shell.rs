use std::fs::File;
use std::io::Write;
use std::os::fd::FromRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, Stdio};

fn available() -> bool {
    Command::new("bwrap")
        .args(["--unshare-all", "--ro-bind", "/", "/", "--", "true"])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn copy_executable(root: &Path, executable: &str) {
    let dependencies = Command::new("ldd").arg(executable).output().unwrap();
    assert!(dependencies.status.success());
    let libraries = String::from_utf8(dependencies.stdout).unwrap();
    for path in std::iter::once(executable)
        .chain(libraries.split_whitespace().filter(|s| s.starts_with('/')))
    {
        let destination = root.join(path.trim_start_matches('/'));
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::copy(path, destination).unwrap();
    }
}

#[test]
fn debug_shell_shares_native_permissions_and_inherits_input_and_terminal() {
    if !available() {
        eprintln!("bubblewrap is unavailable; skip native debug shell execution");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let workspace = directory.path().join("workspace");
    let stage = directory.path().join("stage");
    for path in [&workspace, &stage] {
        std::fs::create_dir(path).unwrap();
    }
    for executable in ["/bin/sh", "/usr/bin/id", "/usr/bin/stat"] {
        copy_executable(&root, executable);
    }
    for path in ["dev", "proc", "sys", "tmp"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    std::fs::write(root.join("root-marker"), "original\n").unwrap();
    let shell = |script: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpmx"));
        command
            .arg("shell")
            .arg("--root")
            .arg(&root)
            .arg("--workspace")
            .arg(&workspace)
            .arg("--stage")
            .arg(&stage)
            .arg("--command")
            .arg(script);
        command
    };
    let script = r#"
set -eux
test "$(id -u)" = 65534
test "$(id -g)" = 65534
test "$(stat -c %a /tmp)" = 1777
sets=0
while read -r key value rest; do
  case $key in
    CapInh:|CapPrm:|CapEff:|CapBnd:|CapAmb:) test "$value" = 0000000000000000; sets=$((sets+1));;
    NoNewPrivs:) test "$value" = 1; sets=$((sets+1));;
  esac
done </proc/self/status
test "$sets" = 6
for map in uid_map gid_map; do
  covered=0
  while read -r inside outside count; do
    # Kernel mappings do not overlap, but their lines need not be sorted.
    if test "$inside" -lt 65536; then
      end=$((inside+count))
      if test "$end" -gt 65536; then end=65536; fi
      covered=$((covered+end-inside))
    fi
  done </proc/self/$map
  test "$covered" -ge 65536
done
if (printf 'changed\n' >/root-marker) 2>/dev/null; then
  echo 'debug root unexpectedly writable' >&2
  exit 1
fi
test "$PREFIX" = /usr
test "$LIBDIR" = /usr/lib64
test "$STAGE" = /tmp/stage
IFS= read -r input
test "$input" = 'inherited standard input'
printf 'workspace\n' >workspace-write
printf 'stage\n' >"$STAGE/stage-write"
"#;
    let mut child = shell(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"inherited standard input\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("root-marker")).unwrap(),
        "original\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("workspace-write")).unwrap(),
        "workspace\n"
    );
    assert_eq!(
        std::fs::read_to_string(stage.join("stage-write")).unwrap(),
        "stage\n"
    );
    assert_eq!(
        std::fs::metadata(workspace.join("workspace-write"))
            .unwrap()
            .uid(),
        unsafe { libc::getuid() }
    );

    // Each standard stream must remain a terminal, rather than a runner pipe.
    let (mut master, mut slave) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    // openpty does not set CLOEXEC; only the three deliberate stdio copies
    // should reach the CLI, not the test's master/slave handles.
    assert_eq!(
        unsafe { libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC) },
        0
    );
    assert_eq!(
        unsafe { libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC) },
        0
    );
    let _master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let status = shell("test -t 0 && test -t 1 && test -t 2")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave))
        .status()
        .unwrap();
    assert!(
        status.success(),
        "debug shell must inherit all terminal streams"
    );
    let failed = shell("exit 37").output().unwrap();
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("exit status: 37"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
}

#[test]
fn native_debug_terminal_uses_host_tools_and_a_readonly_foreign_sysroot() {
    if !available() {
        eprintln!("bubblewrap is unavailable; skip native cross debug execution");
        return;
    }
    use rpmx_core::guest::CrossToolchain;
    use rpmx_core::runner::{RunnerAssets, RunnerKind};
    use rpmx_core::sandbox::{run_native_terminal, SandboxConfig};
    use rpmx_core::Arch;
    use std::num::{NonZeroU32, NonZeroU64};

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let workspace = directory.path().join("workspace");
    let stage = directory.path().join("stage");
    let sysroot = directory.path().join("target");
    for path in [&workspace, &stage, &sysroot] {
        std::fs::create_dir(path).unwrap();
    }
    copy_executable(&root, "/bin/sh");
    for path in ["dev", "proc", "sys", "tmp", "sysroot"] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    std::fs::write(sysroot.join("target-marker"), "target sysroot\n").unwrap();
    let host = Arch::current().unwrap();
    let target = if host == Arch::X86_64 {
        Arch::Aarch64
    } else {
        Arch::X86_64
    };
    let toolchain = CrossToolchain::new(target.triple());
    let config = SandboxConfig {
        writable_root: false,
        runner: RunnerKind::Bwrap,
        host,
        exec: host,
        target,
        root: Some(&root),
        workspace: &workspace,
        stage: &stage,
        sysroot: Some(&sysroot),
        cross: Some(&toolchain),
        assets: RunnerAssets::default(),
        image: None,
        prefix: "/usr",
        libdir: "/usr/lib64",
        source_date_epoch: 0,
        jobs: NonZeroU32::new(1).unwrap(),
        timeout: NonZeroU64::new(1).unwrap(),
    };
    let script = format!(
        r#"
set -eux
test "$RPMX_EXEC_ARCH" = {host}
test "$RPMX_TARGET_ARCH" = {target}
test "$RPMX_CROSS" = 1
test "$RPMX_EMULATED" = 0
test "$RPMX_SYSROOT" = /sysroot
test "$CC" = {triple}-gcc
test -f "$RPMX_CMAKE_TOOLCHAIN_FILE"
test -f "$RPMX_MESON_CROSS_FILE"
IFS= read -r marker </sysroot/target-marker
test "$marker" = 'target sysroot'
if (printf changed >/sysroot/target-marker) 2>/dev/null; then
  echo 'debug sysroot unexpectedly writable' >&2
  exit 1
fi
printf 'native cross tools\n' >cross-write
"#,
        triple = target.triple()
    );
    run_native_terminal(&config, "sh", &["-c".into(), script]).unwrap();
    assert_eq!(
        std::fs::read_to_string(sysroot.join("target-marker")).unwrap(),
        "target sysroot\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("cross-write")).unwrap(),
        "native cross tools\n"
    );
}
