use std::collections::BTreeMap;
use std::process::Command;

use rpmx_core::plan::PlannedStep;
use rpmx_core::sandbox::{Sandbox, SandboxBackend};

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
    Sandbox {
        backend: SandboxBackend::Bwrap,
        workspace: work.path(),
        stage: stage.path(),
        source_date_epoch: 1_700_000_000,
        jobs: 1,
        timeout: None,
        debuginfo: false,
    }
    .run(step)
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
        source_date_epoch: 1_700_000_000,
        jobs: 1,
        timeout: None,
        debuginfo: false,
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
             test \"$SOURCE_DATE_EPOCH\" = 1700000000",
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
        source_date_epoch: 1_700_000_000,
        jobs: 1,
        timeout: Some(1),
        debuginfo: false,
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
