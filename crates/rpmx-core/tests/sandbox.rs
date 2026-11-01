use std::collections::BTreeMap;
use std::process::Command;

use rpmx_core::arch::Arch;
use rpmx_core::plan::PlannedStep;
use rpmx_core::sandbox::{
    Runner, RunnerKind, RunnerRequest, Sandbox, SandboxBackend, Session, SessionConfig,
};

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
    let sandbox = Sandbox {
        backend: SandboxBackend::Bwrap,
        workspace: work.path(),
        stage: stage.path(),
        host_arch: rpmx_core::arch::Arch::current().expect("host arch"),
        target_arch: rpmx_core::arch::Arch::current().expect("target arch"),
        source_date_epoch: 1_700_000_000,
        jobs: 1,
        timeout: None,
        debuginfo: false,
        session: std::cell::RefCell::new(None),
    };
    sandbox.run(step)
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
    let error = Sandbox {
        backend: SandboxBackend::Bwrap,
        workspace: work.path(),
        stage: stage.path(),
        host_arch: host,
        target_arch: target,
        source_date_epoch: 1_700_000_000,
        jobs: 1,
        timeout: None,
        debuginfo: false,
        session: std::cell::RefCell::new(None),
    }
    .run(&step("true", &[]))
    .expect_err("bwrap cannot execute foreign architecture binaries");

    assert_eq!(error.code, "RPMX-RUNNER-002");
    assert!(error.message.contains("use a runner"));
}

struct ProbeRunner {
    seen: std::sync::Arc<std::sync::Mutex<Option<(Arch, Arch, String)>>>,
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

    fn start(&self, config: &SessionConfig<'_>) -> Result<Box<dyn Session>, rpmx_core::Error> {
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
    };
    Sandbox {
        backend: SandboxBackend::Custom(&probe),
        workspace: work.path(),
        stage: stage.path(),
        host_arch: Arch::X86_64,
        target_arch: Arch::Aarch64,
        source_date_epoch: 1_700_000_000,
        jobs: 2,
        timeout: None,
        debuginfo: false,
        session: std::cell::RefCell::new(None),
    }
    .run(&step("true", &[]))
    .expect("custom runner");

    let seen = probe.seen.lock().expect("probe lock").clone();
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

    Sandbox {
        backend: SandboxBackend::Bwrap,
        workspace: work.path(),
        stage: stage.path(),
        host_arch: rpmx_core::arch::Arch::current().expect("host arch"),
        target_arch: rpmx_core::arch::Arch::current().expect("target arch"),
        source_date_epoch: 1_700_000_000,
        jobs: 1,
        timeout: None,
        debuginfo: false,
        session: std::cell::RefCell::new(None),
    }
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
    let error = Sandbox {
        backend: SandboxBackend::Bwrap,
        workspace: work.path(),
        stage: stage.path(),
        host_arch: rpmx_core::arch::Arch::current().expect("host arch"),
        target_arch: rpmx_core::arch::Arch::current().expect("target arch"),
        source_date_epoch: 1_700_000_000,
        jobs: 1,
        timeout: Some(1),
        debuginfo: false,
        session: std::cell::RefCell::new(None),
    }
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
    let host = Arch::current().expect("host arch");
    let sandbox = Sandbox {
        backend: SandboxBackend::Container(image),
        workspace: work.path(),
        stage: stage.path(),
        host_arch: host,
        target_arch: host,
        source_date_epoch: 1_700_000_000,
        jobs: 1,
        timeout: None,
        debuginfo: false,
        session: std::cell::RefCell::new(None),
    };
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

    assert!(matches!(
        RunnerKind::Auto
            .select(host, host, Some(root), None)
            .expect("native auto"),
        SandboxBackend::NativeRoot(_)
    ));
    assert!(matches!(
        RunnerKind::Bwrap
            .select(host, host, Some(root), None)
            .expect("native bwrap"),
        SandboxBackend::NativeRoot(_)
    ));
    assert!(matches!(
        RunnerKind::Auto
            .select(host, host, None, Some("image"))
            .expect("container auto"),
        SandboxBackend::Container(_)
    ));

    let foreign = if host == Arch::X86_64 {
        Arch::Aarch64
    } else {
        Arch::X86_64
    };
    assert!(matches!(
        RunnerKind::Auto
            .select(host, foreign, Some(root), None)
            .expect("foreign auto"),
        SandboxBackend::Qemu(_)
    ));
    let error = match RunnerKind::Bwrap.select(host, foreign, Some(root), None) {
        Ok(_) => panic!("bwrap cannot cross"),
        Err(error) => error,
    };
    assert_eq!(error.code, "RPMX-RUNNER-002");
    let error = match RunnerKind::Qemu.select(host, foreign, None, None) {
        Ok(_) => panic!("qemu needs a root"),
        Err(error) => error,
    };
    assert_eq!(error.code, "RPMX-ROOT-016");
}
