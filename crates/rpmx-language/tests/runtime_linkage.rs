#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Output};

use rpmx_language::steps;

struct Fixture {
    directory: tempfile::TempDir,
    libraries: PathBuf,
    loaders: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let libraries = directory.path().join("libraries with spaces");
        let loaders = directory.path().join("loaders");
        std::fs::create_dir(&libraries).unwrap();
        std::fs::create_dir(&loaders).unwrap();
        let native_loader = ["/lib", "/lib64", "/usr/lib", "/usr/lib64"]
            .into_iter()
            .filter_map(|path| std::fs::read_dir(path).ok())
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("ld-linux-")
                    && path.is_file()
            })
            .expect("native Linux ELF loader");
        std::os::unix::fs::symlink(native_loader, loaders.join("ld-linux-fixture.so.1")).unwrap();
        Self {
            directory,
            libraries,
            loaders,
        }
    }

    fn compile(&self, name: &str, source: &str, arguments: &[&str]) {
        let input = self.directory.path().join(format!("{name}.c"));
        std::fs::write(&input, source).unwrap();
        let output = Command::new("cc")
            .args(["-shared", "-fPIC"])
            .arg(input)
            .args(arguments)
            .arg("-o")
            .arg(self.libraries.join(name))
            .output()
            .expect("run C compiler for native loader fixture");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run(&self, libraries: &[&str]) -> Output {
        let with = BTreeMap::from([("libraries".into(), serde_json::json!(libraries))]);
        let step = steps::resolve("test/runtime-linkage", None, Some(&with), false).unwrap();
        assert_eq!(step.needs_packages, ["bash", "coreutils"]);
        assert!(step.needs_sysroot.is_empty());
        // Point the real step at temporary paths without changing the host's library tree.
        let script = step.args[1]
            .replace("${libdir}", self.libraries.to_str().unwrap())
            .replace(
                "/lib*/ld-linux-*.so.*",
                &format!("{}/*", self.loaders.display()),
            );
        Command::new("bash")
            .args(["--noprofile", "--norc", "-euo", "pipefail", "-c"])
            .arg(script)
            .envs(step.env)
            .env_remove("LD_LIBRARY_PATH")
            .current_dir(self.directory.path())
            .output()
            .unwrap()
    }
}

#[test]
fn runtime_linkage_loads_native_elf_and_ignores_matching_gdb_scripts() {
    let fixture = Fixture::new();
    fixture.compile("libfixture.so.1", "int fixture(void) { return 42; }", &[]);
    std::fs::write(
        fixture.directory.path().join("libfixture.so.99"),
        "an unrelated current-directory glob match",
    )
    .unwrap();
    std::fs::write(
        fixture.libraries.join("libfixture.so.1-gdb.py"),
        "# matching GDB sidecar is not an ELF library\n",
    )
    .unwrap();
    let output = fixture.run(&["libfixture.so.*"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("checking "));
    assert!(stdout.contains("libfixture.so.1"));
    assert!(!stdout.contains("-gdb.py"));
}

#[test]
fn runtime_linkage_requires_an_elf_match_for_every_pattern() {
    let fixture = Fixture::new();
    fixture.compile("libfixture.so.1", "int fixture(void) { return 42; }", &[]);
    for patterns in [
        vec!["libfixture.so.*", "libmissing.so.*"],
        vec!["libsidecar.so.*"],
    ] {
        std::fs::write(
            fixture.libraries.join("libsidecar.so.1-gdb.py"),
            "# not an ELF shared library\n",
        )
        .unwrap();
        let output = fixture.run(&patterns);
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("no ELF shared library matches"), "{stderr}");
        assert!(stderr.contains(patterns.last().unwrap()), "{stderr}");
        assert!(stderr.contains(fixture.libraries.to_str().unwrap()));
    }
}

#[test]
fn runtime_linkage_fails_when_a_needed_library_is_missing() {
    let fixture = Fixture::new();
    fixture.compile(
        "libfixture_dependency.so.1",
        "int dependency(void) { return 42; }",
        &["-Wl,-soname,libfixture_dependency.so.1"],
    );
    fixture.compile(
        "libfixture.so.1",
        "extern int dependency(void); int fixture(void) { return dependency(); }",
        &[
            "-L",
            fixture.libraries.to_str().unwrap(),
            "-l:libfixture_dependency.so.1",
            "-Wl,-rpath,$ORIGIN",
        ],
    );
    let output = fixture.run(&["libfixture.so.*"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_file(fixture.libraries.join("libfixture_dependency.so.1")).unwrap();
    let output = fixture.run(&["libfixture.so.*"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("libfixture_dependency.so.1"), "{stderr}");
    assert!(
        stderr.contains("cannot resolve dependencies of"),
        "{stderr}"
    );
    assert!(stderr.contains("libfixture.so.1"), "{stderr}");
}

#[test]
fn runtime_linkage_fails_without_an_installed_loader() {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.loaders.join("ld-linux-fixture.so.1")).unwrap();
    let output = fixture.run(&["libfixture.so.*"]);
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("no installed ELF loader found"));
}

#[test]
fn runtime_linkage_requires_nonempty_basename_patterns() {
    assert!(steps::resolve("test/runtime-linkage", None, None, false).is_err());
    let fixture = Fixture::new();
    for patterns in [vec![], vec!["../libfixture.so.*"]] {
        let output = fixture.run(&patterns);
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("libraries must contain") || stderr.contains("SONAME basename"),
            "{stderr}"
        );
    }
}
