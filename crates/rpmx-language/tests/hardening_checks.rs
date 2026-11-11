#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use rpmx_language::steps;

struct Fixture {
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let stage = directory.path().join("stage");
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(stage.join("probe"), "controlled ELF fixture").unwrap();
        executable(
            &bin.join("readelf"),
            r#"#!/bin/sh
set -eu
cat "$HARDENING_FIXTURE/$1"
"#,
        );
        executable(
            &bin.join("compiler"),
            r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  if [ "$1" = -o ]; then
    shift
    : > "$1"
    exit 0
  fi
  shift
done
exit 1
"#,
        );
        let tail = "unrelated output line\n".repeat(100_000);
        for (option, content) in [
            ("-h", "  Type: DYN (Position-Independent Executable file)\n"),
            ("-lW", "GNU_STACK RW\nGNU_RELRO\n"),
            ("-dW", "FLAGS: BIND_NOW\n"),
            ("-nW", "IBT SHSTK BTI\nBuild ID: fixture\n"),
            ("-sW", "__stack_chk_fail\n__memcpy_chk@GLIBC_2.3.4\n"),
        ] {
            // Larger than a pipe buffer. Early-exit consumers cause the
            // controlled producer to receive SIGPIPE under strict pipelines.
            std::fs::write(directory.path().join(option), format!("{content}{tail}")).unwrap();
        }
        Self { directory }
    }

    fn run(&self, compiler: bool) -> Output {
        let name = if compiler {
            "test/compiler-hardening-check"
        } else {
            "test/hardening-check"
        };
        let inputs = BTreeMap::from([("cc".to_string(), serde_json::json!("compiler"))]);
        let step = steps::resolve(name, None, compiler.then_some(&inputs), false).unwrap();
        let path = format!(
            "{}:{}",
            self.directory.path().join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        Command::new("bash")
            .args(["--noprofile", "--norc", "-euo", "pipefail", "-c"])
            .arg(&step.args[1])
            .env("PATH", path)
            .env("HARDENING_FIXTURE", self.directory.path())
            .env("STAGE", self.directory.path().join("stage"))
            .env("RPMX_TARGET_ARCH", "x86_64")
            .env_remove("CFLAGS")
            .env_remove("LDFLAGS")
            .current_dir(self.directory.path())
            .output()
            .unwrap()
    }
}

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn staged_hardening_check_drains_large_readelf_output() {
    let output = Fixture::new().run(false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiler_hardening_check_drains_large_readelf_output() {
    let output = Fixture::new().run(true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn missing_hardening_still_fails_with_a_specific_diagnostic() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.directory.path().join("-sW"),
        "no hardening symbols\n",
    )
    .unwrap();
    for compiler in [false, true] {
        let output = fixture.run(compiler);
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("no stack-protector reference"), "{stderr}");
        assert!(stderr.contains("no fortified libc reference"), "{stderr}");
    }
}

#[test]
#[ignore = "set RPMX_HARDENING_STAGE to a staged native hardened ELF tree"]
fn hardening_check_accepts_real_staged_artifacts() {
    let stage = std::env::var("RPMX_HARDENING_STAGE").expect("staged artifact directory");
    let step = steps::resolve("test/hardening-check", None, None, false).unwrap();
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-euo", "pipefail", "-c"])
        .arg(&step.args[1])
        .env("STAGE", stage)
        .env("RPMX_TARGET_ARCH", std::env::consts::ARCH)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
