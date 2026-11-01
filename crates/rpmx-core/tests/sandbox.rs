use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::process::Command;

use rpmx_core::arch::Arch;
use rpmx_core::plan::PlannedStep;
use rpmx_core::runner::{Runner, RunnerConfig, RunnerKind, RunnerRequest, Session};
use rpmx_core::sandbox::{Sandbox, SandboxConfig};

fn config<'a>(workspace: &'a std::path::Path, stage: &'a std::path::Path) -> SandboxConfig<'a> {
    let host = Arch::current().expect("host arch");
    SandboxConfig {
        runner: RunnerKind::Bwrap,
        host,
        target: host,
        workspace,
        stage,
        root: None,
        image: None,
        source_date_epoch: 1_700_000_000,
        jobs: NonZeroU32::new(1).expect("jobs"),
        timeout: None,
        debuginfo: false,
    }
}

fn bwrap_available() -> bool {
    Command::new("bwrap")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn step(command: &str, args: &[&str]) -> PlannedStep {
    PlannedStep {
        uses: "inline".to_string(),
        command: Some(command.to_string()),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: None,
        env: BTreeMap::new(),
        needs: Vec::new(),
        tool_digest: None,
        step_digest: None,
        builtin: false,
        inputs: BTreeMap::new(),
        input: None,
        input_file: None,
    }
}

fn run(step: &PlannedStep) -> Result<(), rpmx_core::Error> {
    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");
    let sandbox = Sandbox::new(config(work.path(), stage.path()))?;
    sandbox.run(step)
}

#[test]
fn constructing_a_foreign_sandbox_does_not_start_a_runner() {
    let host = Arch::current().expect("host arch");
    let target = if host == Arch::X86_64 {
        Arch::Aarch64
    } else {
        Arch::X86_64
    };
    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");

    Sandbox::new(SandboxConfig {
        target,
        ..config(work.path(), stage.path())
    })
    .expect("constructing a sandbox that runs no step must not check the runner");
}

#[test]
fn native_runners_reject_a_foreign_target() {
    let host = Arch::current().expect("host arch");
    let target = if host == Arch::X86_64 {
        Arch::Aarch64
    } else {
        Arch::X86_64
    };
    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");
    let sandbox = Sandbox::new(SandboxConfig {
        target,
        ..config(work.path(), stage.path())
    })
    .expect("sandbox");
    let error = match sandbox.run(&step("true", &[])) {
        Ok(()) => panic!("bwrap cannot execute foreign architecture binaries"),
        Err(error) => error,
    };

    assert_eq!(error.code, "RPMX-RUNNER-002");
    assert!(error.message.contains("use a runner"));
}

struct ProbeRunner {
    seen: std::sync::Arc<std::sync::Mutex<Option<(Arch, Arch, String)>>>,
    started: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

struct ProbeSession {
    seen: std::sync::Arc<std::sync::Mutex<Option<(Arch, Arch, String)>>>,
    host: Arch,
    target: Arch,
}

impl Runner for ProbeRunner {
    fn name(&self) -> &'static str {
        "probe"
    }

    fn supports(&self, _host: Arch, _target: Arch) -> bool {
        true
    }

    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>, rpmx_core::Error> {
        self.started
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(Box::new(ProbeSession {
            seen: self.seen.clone(),
            host: config.host,
            target: config.target,
        }))
    }
}

impl Session for ProbeSession {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<(), rpmx_core::Error> {
        *self.seen.lock().expect("probe lock") = Some((
            self.host,
            self.target,
            request
                .env
                .get("RPMX_TARGET_ARCH")
                .cloned()
                .unwrap_or_default(),
        ));
        Ok(())
    }

    fn sync_out(&mut self) -> Result<(), rpmx_core::Error> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), rpmx_core::Error> {
        Ok(())
    }
}

#[test]
fn a_custom_runner_receives_the_arch_contract() {
    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");
    let probe = ProbeRunner {
        seen: std::sync::Arc::new(std::sync::Mutex::new(None)),
        started: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let seen = probe.seen.clone();
    Sandbox::with_runner(
        SandboxConfig {
            host: Arch::X86_64,
            target: Arch::Aarch64,
            jobs: NonZeroU32::new(2).expect("jobs"),
            ..config(work.path(), stage.path())
        },
        Box::new(probe),
    )
    .run(&step("true", &[]))
    .expect("custom runner step");

    let seen = seen.lock().expect("probe lock").clone();
    assert_eq!(seen, Some((Arch::X86_64, Arch::Aarch64, "aarch64".into())));
}

#[test]
fn runs_an_inline_step_with_a_writable_stage() {
    if !bwrap_available() {
        eprintln!("bwrap not installed; skipping sandbox test");
        return;
    }

    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");
    let step = step(
        "sh",
        &[
            "-c",
            "mkdir -p /tmp/stage/usr/bin && printf 'demo\\n' > /tmp/stage/usr/bin/demo",
        ],
    );

    Sandbox::new(config(work.path(), stage.path()))
        .expect("sandbox")
        .run(&step)
        .expect("sandbox step");

    assert_eq!(
        std::fs::read(stage.path().join("usr/bin/demo")).expect("staged file"),
        b"demo\n"
    );
}

#[test]
fn host_environment_is_not_inherited() {
    if !bwrap_available() {
        eprintln!("bwrap not installed; skipping sandbox test");
        return;
    }
    if std::env::var_os("USER").is_none() {
        eprintln!("USER is not set; skipping environment test");
        return;
    }
    // Do not mutate the process environment: other tests spawn processes in
    // parallel and getenv is not thread-safe. USER is set by the host login.
    run(&step("sh", &["-c", "test -z \"${USER+x}\""]))
        .expect("the host environment must not reach the step");
}

#[test]
fn sandbox_environment_is_reproducible() {
    if !bwrap_available() {
        eprintln!("bwrap not installed; skipping sandbox test");
        return;
    }
    run(&step(
        "sh",
        &[
            "-c",
            "test \"$HOME\" = /tmp/build && test \"$TZ\" = UTC && \
             test \"$LANG\" = C.UTF-8 && test \"$LC_ALL\" = C.UTF-8 && \
             test \"$PATH\" = /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin && \
             test \"$SOURCE_DATE_EPOCH\" = 1700000000 && \
             test \"$RPMX_HOST_ARCH\" = \"$(uname -m | sed 's/arm64/aarch64/')\" && \
             test \"$RPMX_TARGET_ARCH\" = \"$RPMX_HOST_ARCH\" && \
             test \"$RPMX_CROSS\" = 0",
        ],
    ))
    .expect("deterministic environment");
}

#[test]
fn the_sandbox_has_no_network_routes() {
    if !bwrap_available() {
        eprintln!("bwrap not installed; skipping sandbox test");
        return;
    }
    // A new network namespace has only the header line in /proc/net/route.
    run(&step(
        "sh",
        &["-c", "test \"$(wc -l < /proc/net/route)\" -eq 1"],
    ))
    .expect("the sandbox must have its own empty network namespace");
}

#[test]
fn nested_user_namespaces_are_disabled() {
    if !bwrap_available() {
        eprintln!("bwrap not installed; skipping sandbox test");
        return;
    }
    if Command::new("unshare").arg("--version").output().is_err() {
        eprintln!("unshare not installed; skipping nested-userns test");
        return;
    }
    run(&step("sh", &["-c", "unshare -Ur true 2>/dev/null"]))
        .expect_err("a nested user namespace must be refused");
}

#[test]
fn reserved_environment_variables_are_rejected() {
    let mut step = step("true", &[]);
    step.env.insert("STAGE".to_string(), "/etc".to_string());
    let error = run(&step).expect_err("a step must not move the staging tree");
    assert_eq!(error.code, "RPMX-SANDBOX-003");
}

#[test]
fn policy_rejects_reserved_environment_before_starting_a_runner() {
    let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let probe = ProbeRunner {
        seen: std::sync::Arc::new(std::sync::Mutex::new(None)),
        started: started.clone(),
    };
    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");
    let mut step = step("true", &[]);
    step.env
        .insert("SOURCE_DATE_EPOCH".to_string(), "0".to_string());

    let error = Sandbox::with_runner(config(work.path(), stage.path()), Box::new(probe))
        .run(&step)
        .expect_err("policy must reject a reserved variable");

    assert_eq!(error.code, "RPMX-SANDBOX-003");
    assert!(
        !started.load(std::sync::atomic::Ordering::SeqCst),
        "a rejected step must not start a runner session"
    );
}

#[test]
fn a_step_that_overruns_is_killed() {
    if !bwrap_available() {
        eprintln!("bwrap not installed; skipping sandbox test");
        return;
    }
    if Command::new("timeout").arg("--version").output().is_err() {
        eprintln!("timeout not installed; skipping timeout test");
        return;
    }

    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");
    let started = std::time::Instant::now();
    let error = Sandbox::new(SandboxConfig {
        timeout: Some(NonZeroU64::new(1).expect("timeout")),
        ..config(work.path(), stage.path())
    })
    .expect("sandbox")
    .run(&step("sh", &["-c", "sleep 30"]))
    .expect_err("the step must be killed");

    assert_eq!(error.code, "RPMX-SANDBOX-004");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "timeout took {:?}",
        started.elapsed()
    );
}

#[test]
fn podman_session_has_no_network() {
    if Command::new("podman").arg("--version").output().is_err() {
        eprintln!("podman not installed; skipping podman session test");
        return;
    }
    let image = "registry.fedoraproject.org/fedora:44";
    let exists = Command::new("podman")
        .args(["image", "exists", image])
        .status()
        .is_ok_and(|status| status.success());
    if !exists {
        eprintln!("{image} is not local; skipping podman session test");
        return;
    }
    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");
    let sandbox = Sandbox::new(SandboxConfig {
        runner: RunnerKind::Auto,
        image: Some(image),
        ..config(work.path(), stage.path())
    })
    .expect("podman sandbox");
    sandbox
        .run(&step(
            "sh",
            &[
                "-c",
                "test -z \"$(ls /sys/class/net 2>/dev/null | grep -v '^lo$')\"",
            ],
        ))
        .expect("podman container must have no network interfaces");
}

#[test]
fn runner_selection_is_centralized() {
    let host = Arch::current().expect("host arch");
    let root = std::path::Path::new("/root");

    assert_eq!(
        RunnerKind::Auto
            .resolve(host, host, Some(root), None)
            .expect("native auto")
            .name(),
        "bubblewrap"
    );
    assert_eq!(
        RunnerKind::Bwrap
            .resolve(host, host, Some(root), None)
            .expect("native bwrap")
            .name(),
        "bubblewrap"
    );
    assert_eq!(
        RunnerKind::Auto
            .resolve(host, host, None, Some("image"))
            .expect("container auto")
            .name(),
        "podman"
    );

    let foreign = if host == Arch::X86_64 {
        Arch::Aarch64
    } else {
        Arch::X86_64
    };
    assert_eq!(
        RunnerKind::Auto
            .resolve(host, foreign, Some(root), None)
            .expect("foreign auto")
            .name(),
        "qemu-system"
    );
    let error = match RunnerKind::Bwrap.resolve(host, foreign, Some(root), None) {
        Ok(_) => panic!("bwrap cannot cross"),
        Err(error) => error,
    };
    assert_eq!(error.code, "RPMX-RUNNER-002");
    let error = match RunnerKind::Qemu.resolve(host, foreign, None, None) {
        Ok(_) => panic!("qemu needs a root"),
        Err(error) => error,
    };
    assert_eq!(error.code, "RPMX-ROOT-016");
}
