use std::path::{Path, PathBuf};

use super::process::{finish, spawn_inheriting_fds, spawn_with_fds};
use super::user_namespace::UserNamespace;
use crate::runner::{Runner, RunnerConfig, RunnerRequest, Session};
use rpmx_base::arch::Arch;
use rpmx_base::error::Result;
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};

pub struct BwrapBackend<'a> {
    pub root: Option<&'a Path>,
}

impl Runner for BwrapBackend<'_> {
    fn name(&self) -> &'static str {
        "bubblewrap"
    }

    fn supports(&self, host: Arch, exec: Arch) -> bool {
        exec == host || (self.root.is_some() && crate::runner::emulation_available(exec))
    }

    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>> {
        Ok(Box::new(BwrapSession::new(self.root, config)))
    }
}

struct BwrapSession {
    writable_root: bool,
    root: PathBuf,
    sysroot: Option<PathBuf>,
    workspace: PathBuf,
    stage: PathBuf,
}

struct NativeCommand {
    args: Vec<String>,
    namespace: UserNamespace,
}

impl BwrapSession {
    fn new(root: Option<&Path>, config: &RunnerConfig<'_>) -> Self {
        Self {
            root: root.unwrap_or(Path::new("/")).to_path_buf(),
            workspace: config.workspace.to_path_buf(),
            stage: config.stage.to_path_buf(),
            sysroot: config.sysroot.map(Path::to_path_buf),
            writable_root: config.writable_root,
        }
    }

    fn command(&self, request: &RunnerRequest<'_>, transaction: bool) -> Result<NativeCommand> {
        let namespace = UserNamespace::new(transaction)?;
        let mut args = vec![
            "--userns".to_string(),
            namespace.fd().as_raw_fd().to_string(),
            // Bubblewrap keeps sync handles in its monitor and closes them in
            // the payload. The namespace handle must not leak into commands.
            "--sync-fd".to_string(),
            namespace.fd().as_raw_fd().to_string(),
            "--unshare-ipc".to_string(),
            "--unshare-pid".to_string(),
            "--unshare-net".to_string(),
            "--unshare-uts".to_string(),
            "--unshare-cgroup-try".to_string(),
            "--assert-userns-disabled".to_string(),
            "--cap-drop".to_string(),
            "ALL".to_string(),
            "--uid".to_string(),
            namespace.uid.to_string(),
            "--gid".to_string(),
            namespace.gid.to_string(),
            "--die-with-parent".to_string(),
            "--new-session".to_string(),
            "--clearenv".to_string(),
            if self.writable_root {
                "--bind"
            } else {
                "--ro-bind"
            }
            .to_string(),
            self.root.display().to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--tmpfs".to_string(),
            "/sys".to_string(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
            "--chmod".to_string(),
            "1777".to_string(),
            "/tmp".to_string(),
            "--bind".to_string(),
            self.workspace.display().to_string(),
            rpmx_base::guest::BUILD.to_string(),
            "--bind".to_string(),
            self.stage.display().to_string(),
            rpmx_base::guest::STAGE.to_string(),
        ];
        if transaction {
            // A transaction owns a private root. Namespace root must be able
            // to update protected account databases, including mode-000 files.
            args.extend(["--cap-add", "CAP_DAC_OVERRIDE"].map(String::from));
        }
        if let Some(sysroot) = &self.sysroot {
            args.push("--ro-bind".to_string());
            args.push(sysroot.display().to_string());
            args.push(rpmx_base::guest::SYSROOT.to_string());
        }
        args.extend(["--chdir".to_string(), request.cwd.to_string()]);
        for (key, value) in request.env {
            args.push("--setenv".to_string());
            args.push(key.clone());
            args.push(value.clone());
        }
        args.push("--".to_string());
        args.push(request.command.to_string());
        args.extend(request.args.iter().cloned());

        Ok(NativeCommand { args, namespace })
    }

    fn run_with_identity(&mut self, request: &RunnerRequest<'_>, transaction: bool) -> Result<()> {
        let command = self.command(request, transaction)?;
        let completion = spawn_with_fds(
            "bwrap",
            &command.args,
            request.timeout,
            request.label,
            request.logs,
            &[command.namespace.fd()],
        )?;
        finish(completion, request.timeout, request.label, "")
    }
}

/// Share native sandbox construction while inheriting the caller's terminal.
/// Interactive debug sessions deliberately have no step deadline or log pumps.
pub(crate) fn run_bwrap_terminal(
    root: Option<&Path>,
    config: &RunnerConfig<'_>,
    request: &RunnerRequest<'_>,
) -> Result<()> {
    let session = BwrapSession::new(root, config);
    let prepared = session.command(request, config.writable_root)?;
    let mut command = Command::new("bwrap");
    command
        .args(&prepared.args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = spawn_inheriting_fds(command, &[prepared.namespace.fd()])
        .and_then(|mut child| child.wait())
        .map_err(|error| {
            rpmx_base::error::Error::new(
                rpmx_base::error::codes::SANDBOX_001,
                format!("cannot run native debug shell: {error}"),
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(rpmx_base::error::Error::new(
            rpmx_base::error::codes::SANDBOX_002,
            format!("shell exited with {status}"),
        ))
    }
}

impl Session for BwrapSession {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()> {
        self.run_with_identity(request, self.writable_root)
    }

    fn sync_out(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Check the operations required by native stages, including host LSM policy.
pub(crate) fn check_bwrap_environment() -> Result<()> {
    super::user_namespace::check_bwrap_user_namespace()?;
    check_transaction_access(true)
}

fn check_transaction_access(transaction: bool) -> Result<()> {
    use std::collections::BTreeMap;
    use std::num::NonZeroU64;
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().map_err(|error| {
        rpmx_base::error::Error::new(rpmx_base::error::codes::SANDBOX_001, error.to_string())
    })?;
    let workspace = directory.path().join("workspace");
    let stage = directory.path().join("stage");
    let prepare = || -> std::io::Result<()> {
        std::fs::create_dir(&workspace)?;
        std::fs::create_dir(&stage)?;
        let protected = workspace.join("transaction-protected");
        std::fs::write(&protected, "original\n")?;
        std::fs::set_permissions(protected, std::fs::Permissions::from_mode(0o0))
    };
    prepare().map_err(|error| {
        rpmx_base::error::Error::new(rpmx_base::error::codes::SANDBOX_001, error.to_string())
    })?;
    let mut session = BwrapSession {
        root: PathBuf::from("/"),
        workspace,
        stage,
        sysroot: None,
        writable_root: false,
    };
    session.run_with_identity(
        &RunnerRequest {
            command: "/bin/sh",
            args: &["-ec".into(), r#"
                if (IFS= read -r value <transaction-protected &&
                    test "$value" = original &&
                    printf 'changed\n' >transaction-protected &&
                    IFS= read -r value <transaction-protected &&
                    test "$value" = changed); then
                    exit 0
                fi
                echo 'host policy denied transaction CAP_DAC_OVERRIDE access to a private mode-000 file' >&2
                if test -r /proc/self/attr/current; then
                    (IFS= read -r profile </proc/self/attr/current;
                     printf 'sandbox security profile: %s\n' "$profile" >&2) || :
                fi
                exit 1
            "#.into()],
            cwd: rpmx_base::guest::BUILD,
            env: &BTreeMap::new(),
            timeout: NonZeroU64::new(10).unwrap(),
            label: "native transaction capability probe",
            logs: None,
        },
        transaction,
    ).map_err(|error| {
        rpmx_base::error::Error::new(rpmx_base::error::codes::SANDBOX_001, format!(
            "native transaction capability probe failed: {error}; host security policy must allow CAP_DAC_OVERRIDE inside bubblewrap transactions. Ubuntu's bwrap-userns-restrict AppArmor profile denies capabilities to payloads even when user namespaces are enabled; inspect the sandbox AppArmor label and kernel audit log"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::num::NonZeroU64;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    #[test]
    fn doctor_verifies_actual_private_transaction_access() {
        if !Command::new("bwrap")
            .args(["--unshare-all", "--ro-bind", "/", "/", "--", "true"])
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("bubblewrap is unavailable; skip the sandbox execution test");
            return;
        }
        check_bwrap_environment().unwrap();
        let denied = check_transaction_access(false).unwrap_err();
        assert!(denied.to_string().contains("CAP_DAC_OVERRIDE"), "{denied}");
    }

    // Copy only the executables and their loader dependencies into a disposable
    // root. In particular, a writable session must never bind the host root.
    fn copy_executable(root: &Path, executable: &str) {
        let dependencies = Command::new("ldd").arg(executable).output().unwrap();
        assert!(dependencies.status.success());
        let output = String::from_utf8(dependencies.stdout).unwrap();
        for name in std::iter::once(executable).chain(
            output
                .split_whitespace()
                .filter(|word| word.starts_with('/')),
        ) {
            let destination = root.join(name.trim_start_matches('/'));
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(name, destination).unwrap();
        }
    }

    #[test]
    fn private_transactions_can_update_protected_files_and_tmp_is_shared() {
        if !Command::new("bwrap")
            .args(["--unshare-all", "--ro-bind", "/", "/", "--", "true"])
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("bubblewrap is unavailable; skip the sandbox execution test");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        copy_executable(&root, "/bin/sh");
        copy_executable(&root, "/usr/bin/stat");
        for path in ["dev", "proc", "sys", "tmp"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        std::fs::create_dir_all(root.join("etc")).unwrap();
        let protected = root.join("etc/protected");
        std::fs::write(&protected, "original\n").unwrap();
        std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o0)).unwrap();
        let workspace = directory.path().join("build");
        let stage = directory.path().join("stage");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&stage).unwrap();
        let source = directory.path().join("transaction-permissions.c");
        std::fs::write(
            &source,
            r#"
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(void) {
  FILE *security = fopen("/proc/self/attr/current", "r");
  if (security != NULL) {
    char label[4096];
    if (fgets(label, sizeof(label), security) != NULL)
      fprintf(stderr, "transaction security profile: %s\n", label);
    fclose(security);
  }
  struct stat metadata;
  if (stat("/etc/protected", &metadata) != 0) {
    perror("stat transaction mode-000 fixture");
    return 1;
  }
  fprintf(stderr, "transaction uid=%lu gid=%lu; fixture uid=%lu gid=%lu mode=%o\n",
          (unsigned long)getuid(), (unsigned long)getgid(),
          (unsigned long)metadata.st_uid, (unsigned long)metadata.st_gid,
          (unsigned int)(metadata.st_mode & 0777));
  if ((metadata.st_mode & 0777) != 0) {
    fputs("transaction fixture must retain mode 000\n", stderr);
    return 1;
  }
  int descriptor = open("/etc/protected", O_RDWR);
  if (descriptor < 0) {
    perror("transaction CAP_DAC_OVERRIDE open(mode 000, O_RDWR)");
    return 1;
  }
  return close(descriptor) != 0;
}
"#,
        )
        .unwrap();
        let compiled = Command::new("cc")
            .args(["-Wall", "-Wextra", "-Werror"])
            .arg(source)
            .arg("-o")
            .arg(workspace.join("transaction-permissions"))
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut session = BwrapSession {
            writable_root: false,
            root,
            sysroot: None,
            workspace,
            stage,
        };
        let environment = BTreeMap::new();
        let run = |session: &mut BwrapSession, script: &str| {
            session.run(&RunnerRequest {
                timeout: NonZeroU64::new(10).unwrap(),
                command: "/bin/sh",
                args: &["-ec".into(), script.into()],
                cwd: rpmx_base::guest::BUILD,
                env: &environment,
                label: "sandbox permissions",
                logs: None,
            })
        };
        run(
            &mut session,
            r#"
            test "$(/usr/bin/stat -c %a /tmp)" = 1777
            if ./transaction-permissions; then
              echo 'ordinary step opened the protected file for writing' >&2
              exit 1
            fi
            if (read -r value </etc/protected); then
              echo 'ordinary step read the protected file' >&2
              exit 1
            fi
        "#,
        )
        .unwrap();
        session.writable_root = true;
        run(
            &mut session,
            r#"
            while read -r key value rest; do
              case $key in
                CapInh:|CapPrm:|CapEff:|CapBnd:|CapAmb:)
                  test "$value" = 0000000000000002 || {
                    echo "unexpected transaction $key $value" >&2
                    exit 1
                  } ;;
                NoNewPrivs:) test "$value" = 1 ;;
              esac
            done </proc/self/status
            ./transaction-permissions
            read -r value </etc/protected
            test "$value" = original
            printf 'changed\n' >/etc/protected
            if (printf 10 >/proc/sys/user/max_user_namespaces); then
              echo 'transaction re-enabled nested user namespaces' >&2
              exit 1
            fi
        "#,
        )
        .unwrap();
        std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read_to_string(protected).unwrap(), "changed\n");
    }

    #[test]
    fn full_id_maps_preserve_real_permissions_and_cannot_restore_privileges() {
        if !Command::new("bwrap")
            .args(["--unshare-all", "--ro-bind", "/", "/", "--", "true"])
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("bubblewrap is unavailable; skip the sandbox execution test");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        let workspace = directory.path().join("build");
        let stage = directory.path().join("stage");
        for path in [&workspace, &stage] {
            std::fs::create_dir(path).unwrap();
        }
        copy_executable(&root, "/bin/sh");
        for path in ["dev", "proc", "sys", "tmp"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        let source = directory.path().join("permissions.c");
        std::fs::write(
            &source,
            r#"
#define _GNU_SOURCE
#include <assert.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <unistd.h>

int main(int argc, char **argv) {
  assert(argc == 2 && getuid() == strtoul(argv[1], NULL, 10) && getuid() != 0);
  assert(prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1);
  FILE *status = fopen("/proc/self/status", "r");
  assert(status);
  char line[512];
  int capability_sets = 0;
  while (fgets(line, sizeof(line), status)) {
    if (strncmp(line, "Cap", 3) == 0) {
      char *value = strchr(line, ':');
      assert(value && strtoull(value + 1, NULL, 16) == 0);
      capability_sets++;
    }
  }
  assert(capability_sets == 5);
  fclose(status);
  errno = 0;
  assert(setuid(0) == -1 && errno == EPERM);
  errno = 0;
  assert(setuid(1) == -1 && errno == EPERM);
  errno = 0;
  assert(setgid(0) == -1 && errno == EPERM);
  errno = 0;
  assert(setresuid(65533, 65533, 65533) == -1 && errno == EPERM);
  assert(unshare(CLONE_NEWUSER) == -1);
  int own_namespace = open("/proc/self/ns/user", O_RDONLY);
  assert(own_namespace >= 0);
  assert(setns(own_namespace, CLONE_NEWUSER) == -1);
  close(own_namespace);
  DIR *descriptors = opendir("/proc/self/fd");
  assert(descriptors);
  struct dirent *entry;
  while ((entry = readdir(descriptors))) {
    char path[512], target[512];
    snprintf(path, sizeof(path), "/proc/self/fd/%s", entry->d_name);
    ssize_t length = readlink(path, target, sizeof(target) - 1);
    if (length < 0) continue;
    target[length] = '\0';
    assert(strncmp(target, "user:[", 6) != 0);
  }
  closedir(descriptors);
  FILE *output = fopen("workspace-write", "w");
  assert(output && fputs("ordinary caller\n", output) >= 0 && fclose(output) == 0);
  return 0;
}
"#,
        )
        .unwrap();
        let executable = workspace.join("permissions");
        let compiled = Command::new("cc")
            .args(["-Wall", "-Wextra", "-Werror"])
            .arg(source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut session = BwrapSession {
            writable_root: false,
            root,
            workspace: workspace.clone(),
            stage,
            sysroot: None,
        };
        let environment = BTreeMap::new();
        let uid = super::super::user_namespace::UNPRIVILEGED_ID.to_string();
        session
            .run(&RunnerRequest {
                timeout: NonZeroU64::new(10).unwrap(),
                command: "./permissions",
                args: &[uid],
                cwd: rpmx_base::guest::BUILD,
                env: &environment,
                label: "full ID map permission semantics",
                logs: None,
            })
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("workspace-write")).unwrap(),
            "ordinary caller\n"
        );
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(workspace.join("workspace-write"))
                .unwrap()
                .uid(),
            unsafe { libc::getuid() }
        );
        copy_executable(&session.root, "/usr/bin/sleep");
        let timeout = session
            .run(&RunnerRequest {
                timeout: NonZeroU64::new(1).unwrap(),
                command: "/bin/sh",
                args: &[
                    "-ec".into(),
                    "/usr/bin/sleep 2; printf escaped >timeout-escaped".into(),
                ],
                cwd: rpmx_base::guest::BUILD,
                env: &environment,
                label: "namespace timeout cleanup",
                logs: None,
            })
            .unwrap_err();
        assert!(timeout.to_string().contains("timed out"), "{timeout}");
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(
            !workspace.join("timeout-escaped").exists(),
            "timed-out commands must not keep running"
        );
    }
}
