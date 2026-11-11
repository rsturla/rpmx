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
option=$1
for argument in "$@"; do file=$argument; done
fixture="$HARDENING_FIXTURE/$option.${file##*/}"
if [ ! -f "$fixture" ]; then fixture="$HARDENING_FIXTURE/$option"; fi
cat "$fixture"
"#,
        );
        executable(
            &bin.join("compiler"),
            r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  if [ "$1" = -o ]; then
    shift
    cat > "$1" <<'PROGRAM'
#!/bin/sh
printf 'hardening\n'
PROGRAM
    chmod +x "$1"
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
            ("-nW", "IBT SHSTK BTI PAC\nBuild ID: fixture\n"),
            ("-sW", "__stack_chk_fail\n__memcpy_chk@GLIBC_2.3.4\n"),
        ] {
            // Larger than a pipe buffer. Early-exit consumers cause the
            // controlled producer to receive SIGPIPE under strict pipelines.
            std::fs::write(directory.path().join(option), format!("{content}{tail}")).unwrap();
        }
        Self { directory }
    }

    fn run(&self, compiler: bool) -> Output {
        self.run_for_arch(compiler, "x86_64")
    }

    fn run_for_arch(&self, compiler: bool, arch: &str) -> Output {
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
            .env("TEST_PACKAGE_ROOT", self.directory.path().join("stage"))
            .env("RPMX_TARGET_ARCH", arch)
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
fn package_hardening_check_drains_large_readelf_output() {
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
fn compiler_branch_checks_use_wrapper_code_instead_of_dependency_startup() {
    let fixture = Fixture::new();
    for file in ["hello", "libprobe.so"] {
        std::fs::write(
            fixture.directory.path().join(format!("-nW.{file}")),
            "Build ID: fixture\n",
        )
        .unwrap();
    }
    for arch in ["x86_64", "aarch64"] {
        let output = fixture.run_for_arch(true, arch);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn missing_branch_protection_in_controlled_final_links_still_fails() {
    let fixture = Fixture::new();
    for file in ["branch-probe", "libbranch-probe.so"] {
        std::fs::write(
            fixture.directory.path().join(format!("-nW.{file}")),
            "Build ID: fixture\n",
        )
        .unwrap();
    }
    for (arch, feature) in [("x86_64", "x86 IBT"), ("aarch64", "aarch64 BTI")] {
        let output = fixture.run_for_arch(true, arch);
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        for file in ["branch-probe", "libbranch-probe.so"] {
            assert!(
                stderr.contains(&format!("{file} has no {feature} property")),
                "{stderr}"
            );
        }
    }
}

#[test]
fn aarch64_bti_alone_cannot_satisfy_return_signing_policy() {
    let fixture = Fixture::new();
    for file in ["branch-probe", "libbranch-probe.so"] {
        std::fs::write(
            fixture.directory.path().join(format!("-nW.{file}")),
            "BTI\nBuild ID: fixture\n",
        )
        .unwrap();
    }
    let output = fixture.run_for_arch(true, "aarch64");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    for file in ["branch-probe", "libbranch-probe.so"] {
        assert!(
            stderr.contains(&format!("{file} has no aarch64 PAC property")),
            "{stderr}"
        );
    }
    assert!(!stderr.contains("has no aarch64 BTI property"), "{stderr}");
}

#[test]
fn ordinary_startup_link_still_requires_nonexecutable_stack_and_relro() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.directory.path().join("-lW.hello"),
        "GNU_STACK RWE\n",
    )
    .unwrap();
    let output = fixture.run(true);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("hello has an executable stack"), "{stderr}");
    assert!(
        stderr.contains("hello has no GNU_RELRO segment"),
        "{stderr}"
    );
}

#[test]
fn ordinary_executable_must_produce_expected_runtime_output() {
    let fixture = Fixture::new();
    let compiler = fixture.directory.path().join("bin/compiler");
    let script = std::fs::read_to_string(&compiler)
        .unwrap()
        .replace("printf 'hardening", "printf 'unexpected");
    executable(&compiler, &script);
    let output = fixture.run(true);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("hello returned unexpected output"),
        "{stderr}"
    );
}

#[test]
fn ordinary_executable_cannot_fail_after_printing_expected_output() {
    let fixture = Fixture::new();
    let compiler = fixture.directory.path().join("bin/compiler");
    let script = std::fs::read_to_string(&compiler)
        .unwrap()
        .replace("printf 'hardening\\n'", "printf 'hardening\\n'\nexit 42");
    executable(&compiler, &script);
    let output = fixture.run(true);
    assert_eq!(output.status.code(), Some(42));
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
#[ignore = "set RPMX_HARDENING_PACKAGE_ROOT to a candidate RPM payload containing native hardened ELFs"]
fn hardening_check_accepts_real_package_artifacts() {
    let stage = std::env::var("RPMX_HARDENING_PACKAGE_ROOT").expect("candidate payload directory");
    let step = steps::resolve("test/hardening-check", None, None, false).unwrap();
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-euo", "pipefail", "-c"])
        .arg(&step.args[1])
        .env("TEST_PACKAGE_ROOT", stage)
        .env("RPMX_TARGET_ARCH", std::env::consts::ARCH)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
