use std::process::{Command, Output};

fn rpmx_in(directory: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .current_dir(directory)
        .args(args)
        .output()
        .expect("run rpmx")
}

#[test]
fn init_creates_a_package_that_lints_clean() {
    let directory = tempfile::tempdir().expect("tempdir");
    let target = directory.path().join("hello");
    let output = rpmx_in(directory.path(), &["init", target.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert!(target.join("package.yaml").is_file());
    assert!(target.join("profiles/fedora-44.yaml").is_file());
    assert!(target.join("README.md").is_file());
    assert!(target.join(".gitignore").is_file());
    let lint = rpmx_in(&target, &["lint"]);
    assert!(lint.status.success(), "{lint:?}");
    assert_eq!(
        String::from_utf8_lossy(&lint.stdout).trim(),
        "ok: 1 cell(s): default"
    );
}

#[test]
fn init_uses_the_requested_profile_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let target = directory.path().join("hello");
    let output = rpmx_in(
        directory.path(),
        &["init", target.to_str().unwrap(), "--profile", "rhel-9"],
    );
    assert!(output.status.success(), "{output:?}");
    let recipe = std::fs::read_to_string(target.join("package.yaml")).expect("recipe");
    assert!(recipe.contains("profile: rhel-9"), "{recipe}");
    assert!(target.join("profiles/rhel-9.yaml").is_file());
}

#[test]
fn init_refuses_to_overwrite_without_force() {
    let directory = tempfile::tempdir().expect("tempdir");
    let target = directory.path().join("hello");
    assert!(
        rpmx_in(directory.path(), &["init", target.to_str().unwrap()])
            .status
            .success()
    );
    let again = rpmx_in(directory.path(), &["init", target.to_str().unwrap()]);
    assert_eq!(again.status.code(), Some(2), "{again:?}");
    let forced = rpmx_in(
        directory.path(),
        &["init", target.to_str().unwrap(), "--force"],
    );
    assert!(forced.status.success(), "{forced:?}");
}

#[test]
fn every_template_lints_clean() {
    let directory = tempfile::tempdir().expect("tempdir");
    for template in ["c", "python", "metapackage"] {
        let target = directory.path().join(format!("{template}-pkg"));
        let output = rpmx_in(
            directory.path(),
            &["init", target.to_str().unwrap(), "--template", template],
        );
        assert!(output.status.success(), "{template}: {output:?}");
        let lint = rpmx_in(&target, &["lint"]);
        assert!(lint.status.success(), "{template}: {lint:?}");
    }
}

#[test]
fn init_can_split_the_recipe_into_sections() {
    let directory = tempfile::tempdir().expect("tempdir");
    let target = directory.path().join("split-hello");
    let output = rpmx_in(
        directory.path(),
        &["init", target.to_str().unwrap(), "--split"],
    );
    assert!(output.status.success(), "{output:?}");
    let recipe = std::fs::read_to_string(target.join("package.yaml")).expect("recipe");
    assert!(recipe.contains("include:"), "{recipe}");
    for section in ["build.yaml", "outputs.yaml", "source-assembly.yaml"] {
        assert!(target.join(section).is_file(), "missing {section}");
    }
    let build = std::fs::read_to_string(target.join("build.yaml")).expect("build section");
    assert!(build.contains("tests:"), "source checks missing: {build}");
    let outputs = std::fs::read_to_string(target.join("outputs.yaml")).expect("outputs section");
    assert!(
        outputs.contains("tests:"),
        "acceptance checks missing: {outputs}"
    );
    let lint = rpmx_in(&target, &["lint"]);
    assert!(lint.status.success(), "{lint:?}");
}
