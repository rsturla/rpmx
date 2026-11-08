use rpmx_core::parse_recipe;
use rpmx_core::plan;

fn plan_with(pipeline: &str) -> rpmx_core::Plan {
    let yaml = format!(
        r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
build:
  pipeline:
{pipeline}
outputs:
  - name: demo
    files:
      include: ["/usr/bin/**"]
"#
    );
    let recipe = parse_recipe(&yaml).expect("recipe");
    plan(&recipe, "x86_64").expect("plan")
}

#[test]
fn cmake_runs_all_phases_from_yaml() {
    let plan = plan_with("    - uses: cmake\n      with:\n        args: [\"-DBUILD_TESTING=ON\"]");
    let steps = &plan.cells[0].pipeline;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].command.as_deref(), Some("sh"));
    let script = &steps[0].args[1];
    assert!(script.contains("cmake -S . -B build"));
    assert!(script.contains("-DBUILD_TESTING=ON"));
    assert!(script.contains("cmake --build build"));
    assert!(script.contains("ctest"));
    assert!(script.contains("DESTDIR=\"$STAGE\" cmake --install"));
    assert!(steps[0].builtin);
}

#[test]
fn meson_runs_all_phases_from_yaml() {
    let plan = plan_with("    - uses: meson");
    let steps = &plan.cells[0].pipeline;
    assert_eq!(steps.len(), 1);
    let script = &steps[0].args[1];
    assert!(script.contains("meson setup build"));
    assert!(script.contains("meson compile -C build"));
    assert!(script.contains("meson install -C build --destdir \"$STAGE\""));
}

#[test]
fn python_builds_and_installs_from_yaml() {
    let plan = plan_with("    - uses: python\n      with:\n        python: /usr/bin/python3.12");
    let steps = &plan.cells[0].pipeline;
    assert_eq!(steps.len(), 1);
    let script = &steps[0].args[1];
    assert!(script.contains("/usr/bin/python3.12 -m build --wheel"));
    assert!(script.contains("pip install --root \"$STAGE\""));
}

#[test]
fn go_build_uses_the_declared_output() {
    let plan = plan_with(
        "    - uses: go/build\n      with:\n        output: /usr/bin/hello\n        args: [-trimpath]",
    );
    let step = &plan.cells[0].pipeline[0];
    assert_eq!(step.command.as_deref(), Some("sh"));
    let script = &step.args[1];
    assert!(script.contains("go build -trimpath"));
    assert!(script.contains("-o \"$STAGE/usr/bin/hello\""));
}

#[test]
fn go_build_can_disable_cgo() {
    let plan = plan_with(
        "    - uses: go/build\n      with:\n        output: /usr/bin/hello\n        cgo: false",
    );
    let script = &plan.cells[0].pipeline[0].args[1];
    assert!(script.contains("CGO_ENABLED=$cgo"), "{script}");
    // The input renders into the guard, so the step actually sees `false`.
    assert!(script.contains("[ \"false\" = true ]"), "{script}");
}

#[test]
fn build_environment_is_applied_to_steps() {
    let yaml = r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
build:
  environment:
    CGO_ENABLED: "0"
    GOCACHE: /tmp/go-cache
  pipeline:
    - uses: go/build
      with:
        output: /usr/bin/demo
outputs:
  - name: demo
    files:
      include: ["/usr/bin/**"]
"#;
    let recipe = parse_recipe(yaml).expect("recipe");
    let plan = plan(&recipe, "x86_64").expect("plan");
    let step = &plan.cells[0].pipeline[0];
    assert_eq!(step.env.get("CGO_ENABLED").map(String::as_str), Some("0"));
    assert_eq!(
        step.env.get("GOCACHE").map(String::as_str),
        Some("/tmp/go-cache")
    );
}

#[test]
fn hardening_check_walks_the_stage_and_honours_skips() {
    let plan = plan_with(
        "    - uses: test/hardening-check\n      with:\n        paths: [\"/usr/bin/demo\"]\n        skip: [fortify]",
    );
    let step = &plan.cells[0].pipeline[0];
    assert_eq!(step.command.as_deref(), Some("sh"));
    let script = &step.args[1];
    assert!(script.contains("readelf -h"), "{script}");
    assert!(script.contains("GNU_RELRO"), "{script}");
    assert!(script.contains("BIND_NOW"), "{script}");
    assert!(script.contains("__stack_chk_fail"), "{script}");
    assert!(script.contains("skip=\" fortify \""), "{script}");
    assert!(script.contains("no fortified libc reference"), "{script}");
    assert!(script.contains("ELF file(s) passed"), "{script}");
    assert!(step.builtin);
}

#[test]
fn debuginfo_keeps_debug_sections_and_fails_when_there_are_none() {
    let plan = plan_with("    - uses: debuginfo");
    let script = &plan.cells[0].pipeline[0].args[1];
    assert!(script.contains("--only-keep-debug"), "{script}");
    assert!(script.contains("readelf"), "{script}");
    assert!(script.contains("--add-gnu-debuglink"), "{script}");
    assert!(
        script.contains("no staged ELF contained debug sections"),
        "{script}"
    );
}
