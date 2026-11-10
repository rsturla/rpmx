use std::fs;
use std::path::Path;

mod common;

use common::plan_with_root;
use rpmx_core::{parse_recipe, plan_with_options, PlanOptions};

fn write_step(root: &Path, name: &str, body: &str) {
    let path = root.join(format!("{name}.yaml"));
    fs::create_dir_all(path.parent().expect("step has a parent")).expect("create step dir");
    fs::write(path, body).expect("write step");
}

fn recipe() -> &'static str {
    r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
source-assembly:
  pipeline:
    - uses: acme/strip
      with:
        path: /usr/bin/foo
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#
}

const STRIP_STEP: &str = r#"
name: acme/strip
version: 1
summary: Strip a binary
inputs:
  path:
    type: string
    required: true
needs:
  packages: [binutils]
run:
  cwd: "${source.dir}"
  argv: ["strip", "--strip-unneeded", "${inputs.path}"]
"#;

#[test]
fn local_step_resolves_and_expands_inputs() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(dir.path(), "acme/strip", STRIP_STEP);

    let plan = plan_with_root(recipe(), dir.path()).expect("local step plans");
    let source = plan.source_assembly.expect("source assembly");
    let step = &source.steps[0];

    assert_eq!(step.uses, "acme/strip");
    assert_eq!(step.command.as_deref(), Some("strip"));
    assert_eq!(step.args, ["--strip-unneeded", "/usr/bin/foo"]);
    assert_eq!(step.cwd.as_deref(), Some("${source.dir}"));
    assert_eq!(step.needs, ["binutils"]);
    assert!(step
        .step_digest
        .as_deref()
        .is_some_and(|digest| digest.starts_with("sha512:")));
}

#[test]
fn local_step_expands_inputs_in_both_dependency_roots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = STRIP_STEP
        .replace("inputs:\n", "inputs:\n  tool:\n    type: string\n    default: binutils-16\n  headers:\n    type: string\n    default: glibc-2.44-devel\n")
        .replace("packages: [binutils]", "packages: ['${inputs.tool}']\n  sysroot: ['${inputs.headers}']");
    write_step(dir.path(), "acme/strip", &source);
    let plan = plan_with_root(recipe(), dir.path()).expect("step plans");
    let source = plan.source_assembly.expect("source assembly");
    assert_eq!(source.steps[0].needs, ["binutils-16", "glibc-2.44-devel"]);
}

#[test]
fn python_wheel_frontend_does_not_require_unrelated_backends() {
    let step = rpmx_core::steps::resolve(
        "python/build-wheel",
        None,
        Some(&std::collections::BTreeMap::from([
            ("python".into(), serde_json::json!("python3.14")),
            (
                "build-package".into(),
                serde_json::json!("python-3.14-build"),
            ),
        ])),
        false,
    )
    .expect("wheel step");
    assert_eq!(step.needs, ["python-3.14-build"]);
    assert_eq!(step.command, "python3.14");
    assert!(step.args.iter().any(|arg| arg == "--no-isolation"));
}

#[test]
fn local_step_requires_declared_inputs() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(dir.path(), "acme/strip", STRIP_STEP);
    let yaml = recipe().replace("      with:\n        path: /usr/bin/foo\n", "");

    let error = plan_with_root(&yaml, dir.path()).expect_err("missing input fails");
    assert_eq!(error.code, "RPMX-STEP-015");
}

#[test]
fn local_step_rejects_unknown_inputs() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(dir.path(), "acme/strip", STRIP_STEP);
    let yaml = recipe().replace("path: /usr/bin/foo", "pth: /usr/bin/foo");

    let error = plan_with_root(&yaml, dir.path()).expect_err("unknown input fails");
    assert_eq!(error.code, "RPMX-STEP-014");
}

#[test]
fn local_step_rejects_path_traversal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let yaml = recipe().replace("acme/strip", "../evil");

    let error = plan_with_root(&yaml, dir.path()).expect_err("traversal fails");
    assert_eq!(error.code, "RPMX-STEP-010");
}

#[test]
fn missing_local_step_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = plan_with_root(recipe(), dir.path()).expect_err("missing step fails");
    assert_eq!(error.code, "RPMX-STEP-011");
}

#[test]
fn local_step_cannot_shadow_a_builtin() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(
        dir.path(),
        "go/bump",
        STRIP_STEP.replace("acme/strip", "go/bump").as_str(),
    );
    let yaml = recipe().replace("acme/strip", "go/bump");

    let error = plan_with_root(&yaml, dir.path()).expect_err("shadowing fails");
    assert_eq!(error.code, "RPMX-STEP-016");
}

#[test]
fn local_step_works_in_the_build_pipeline() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(dir.path(), "acme/strip", STRIP_STEP);
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
build:
  pipeline:
    - uses: acme/strip
      with:
        path: /stage/usr/bin/foo
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    let plan = plan_with_root(yaml, dir.path()).expect("build step plans");
    let step = &plan.cells[0].pipeline[0];
    assert_eq!(step.uses, "acme/strip");
    assert_eq!(step.command.as_deref(), Some("strip"));
    assert_eq!(step.args, ["--strip-unneeded", "/stage/usr/bin/foo"]);
    assert!(step.step_digest.is_some());
}

#[test]
fn local_step_runs_expands_inputs() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(
        dir.path(),
        "acme/shell",
        r#"
name: acme/shell
version: 1
summary: Run a shell step
inputs:
  greeting:
    type: string
    default: hello
run:
  runs: "echo ${inputs.greeting} ${source.dir}"
"#,
    );
    let yaml = recipe()
        .replace("acme/strip", "acme/shell")
        .replace("      with:\n        path: /usr/bin/foo\n", "");

    let plan = plan_with_root(&yaml, dir.path()).expect("shell step plans");
    let step = &plan.source_assembly.expect("source assembly").steps[0];
    assert_eq!(step.command.as_deref(), Some("sh"));
    assert_eq!(step.args, ["-c", "echo hello ${source.dir}"]);
}

#[test]
fn local_step_is_the_same_in_build_and_test_pipelines() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(dir.path(), "acme/strip", STRIP_STEP);
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
build:
  pipeline:
    - uses: acme/strip
      with:
        path: /stage/usr/bin/foo
tests:
  - name: smoke
    stage: installed
    pipeline:
      - uses: acme/strip
        with:
          path: /stage/usr/bin/bar
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    let plan = plan_with_root(yaml, dir.path()).expect("build and test steps plan");
    let build = &plan.cells[0].pipeline[0];
    let test = &plan.cells[0].tests[0].pipeline[0];

    assert_eq!(build.command.as_deref(), Some("strip"));
    assert_eq!(test.command.as_deref(), Some("strip"));
    assert_eq!(test.args, ["--strip-unneeded", "/stage/usr/bin/bar"]);
    assert_eq!(build.step_digest, test.step_digest);
}

#[test]
fn unknown_build_step_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
build:
  pipeline:
    - uses: go/bumpp
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    let error = plan_with_root(yaml, dir.path()).expect_err("typo fails");
    assert_eq!(error.code, "RPMX-STEP-011");
}

#[test]
fn srpm_steps_can_override_embedded_builtins() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(
        dir.path(),
        "go/build",
        r#"
name: go/build
version: 1
summary: local override for the test
inputs:
  output:
    type: string
    default: "${prefix}/bin/foo"
run:
  runs: echo local
"#,
    );
    let recipe = parse_recipe(
        r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
build:
  pipeline:
    - uses: go/build
      with:
        output: /usr/bin/foo
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    )
    .expect("recipe");
    let strict = plan_with_options(
        &recipe,
        PlanOptions {
            arch: "x86_64",
            step_root: Some(dir.path()),
            allow_builtin_override: false,
            dist_tag: None,
            build_release: None,
        },
    )
    .expect_err("shadowing fails without override");
    assert_eq!(strict.code, "RPMX-STEP-016");

    let plan = plan_with_options(
        &recipe,
        PlanOptions {
            arch: "x86_64",
            step_root: Some(dir.path()),
            allow_builtin_override: true,
            dist_tag: None,
            build_release: None,
        },
    )
    .expect("override plans");
    let step = &plan.cells[0].pipeline[0];
    assert!(!step.builtin);
    assert!(step.args[1].contains("echo local"));
}

#[test]
fn local_step_rejects_a_hardcoded_guest_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(
        dir.path(),
        "acme/strip",
        r#"
name: acme/strip
version: 1
summary: Strip a binary
inputs:
  path:
    type: string
    required: true
run:
  argv: ["strip", "--strip-unneeded", "${inputs.path}", "/tmp/build/keep"]
"#,
    );

    let error = plan_with_root(recipe(), dir.path()).expect_err("hardcoded path fails");
    assert_eq!(error.code, "RPMX-STEP-019");
    assert!(error.message.contains("/tmp/build"), "{}", error.message);
}

#[test]
fn local_step_rejects_a_hardcoded_guest_path_in_an_input_default() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_step(
        dir.path(),
        "acme/strip",
        r#"
name: acme/strip
version: 1
summary: Strip a binary
inputs:
  path:
    type: string
    default: "/tmp/stage/usr/bin/foo"
run:
  argv: ["strip", "--strip-unneeded", "${inputs.path}"]
"#,
    );
    let yaml = recipe().replace("      with:\n        path: /usr/bin/foo\n", "");

    let error = plan_with_root(&yaml, dir.path()).expect_err("hardcoded default fails");
    assert_eq!(error.code, "RPMX-STEP-019");
}
