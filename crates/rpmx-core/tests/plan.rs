use rpmx_core::{parse_recipe, plan, plan_with_options, Plan, PlanOptions};

fn plan_str(yaml: &str) -> Result<rpmx_core::Plan, rpmx_core::Error> {
    let recipe = parse_recipe(yaml)?;
    plan(&recipe, "x86_64")
}

fn plan_with_policy(
    yaml: &str,
    dist_tag: Option<&str>,
    build_release: Option<u32>,
) -> Result<Plan, rpmx_core::Error> {
    let recipe = parse_recipe(yaml)?;
    plan_with_options(
        &recipe,
        PlanOptions {
            arch: "x86_64",
            step_root: None,
            allow_builtin_override: false,
            dist_tag,
            build_release,
        },
    )
}

fn error_code(yaml: &str) -> &'static str {
    plan_str(yaml)
        .expect_err("recipe should fail")
        .code
        .as_str()
}

const SHA512: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn minimal(outputs: &str) -> String {
    format!(
        r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
{outputs}
"#
    )
}

#[test]
fn python_matrix_expands_three_cells_and_templates() {
    let yaml = include_str!("../../../examples/python-matrix/package.yaml");
    let plan = plan_str(yaml).expect("example plans");

    let ids: Vec<_> = plan.cells.iter().map(|cell| cell.id.as_str()).collect();
    assert_eq!(ids, ["python=3.10", "python=3.11", "python=3.12"]);

    let first = &plan.cells[0];
    assert_eq!(first.slug, "py310");
    assert_eq!(first.release, "1.py310");
    assert_eq!(first.outputs[0].name, "python3.10-foo");
    assert_eq!(
        first.outputs[0].files,
        [
            "/usr/lib/python3.10/site-packages/foo/**",
            "/usr/lib/python3.10/site-packages/foo-*.dist-info/**"
        ]
    );

    assert_eq!(first.pipeline.len(), 2);
    assert_eq!(first.pipeline[0].uses, "python/build-wheel");
    assert_eq!(
        first.pipeline[0]
            .inputs
            .get("python")
            .and_then(|value| value.as_str()),
        Some("/usr/bin/python3.10")
    );
}

#[test]
fn unknown_field_is_rejected() {
    let yaml = minimal(
        r#"
  nope: true
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-YAML-001");
}

#[test]
fn anchors_and_aliases_are_rejected() {
    let yaml = r#"
schema: 1
package:
  name: &name foo
  version: "1.0"
  release: 1
  summary: *name
  license: MIT
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    assert_eq!(error_code(yaml), "RPMX-YAML-003");
}

#[test]
fn map_axis_requires_value() {
    let yaml = minimal(
        r#"
matrix:
  axes:
    python:
      - tag: "310"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-MATRIX-007");
}

#[test]
fn exclude_removes_matching_cell() {
    let yaml = minimal(
        r#"
matrix:
  axes:
    python:
      - value: "3.10"
        slug: py310
      - value: "3.11"
        slug: py311
      - value: "3.12"
        slug: py312
  exclude:
    - python: "3.11"
outputs:
  - name: "foo-${matrix.python}"
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    let plan = plan_str(&yaml).expect("exclude is valid");
    let ids: Vec<_> = plan.cells.iter().map(|cell| cell.id.as_str()).collect();
    assert_eq!(ids, ["python=3.10", "python=3.12"]);
}

#[test]
fn exclude_referencing_unknown_axis_is_rejected() {
    let yaml = minimal(
        r#"
matrix:
  axes:
    python:
      - value: "3.10"
  exclude:
    - pythn: "3.10"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-MATRIX-010");
}

#[test]
fn max_cells_is_enforced() {
    let yaml = minimal(
        r#"
matrix:
  max-cells: 2
  axes:
    python:
      - value: "3.10"
      - value: "3.11"
      - value: "3.12"
outputs:
  - name: "foo-${matrix.python}"
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-MATRIX-003");
}

#[test]
fn template_cycle_is_rejected() {
    let yaml = minimal(
        r#"
vars:
  a: "${vars.b}"
  b: "${vars.a}"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-TEMPLATE-001");
}

#[test]
fn unknown_template_in_pipeline_is_rejected() {
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
    - uses: python/build-wheel
      with:
        python: /usr/bin/python${matrix.python.version}
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    assert_eq!(error_code(yaml), "RPMX-TEMPLATE-005");
}

#[test]
fn empty_argv_step_is_rejected() {
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
    - argv: []
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    assert_eq!(error_code(yaml), "RPMX-STEP-002");
}

#[test]
fn source_digest_must_be_sha512() {
    let md5 = minimal(
        r#"
sources:
  - name: foo
    kind: archive
    digest: "md5:deadbeef"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&md5), "RPMX-SOURCE-001");

    let short = minimal(
        r#"
sources:
  - name: foo
    kind: archive
    digest: "sha512:abc"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&short), "RPMX-SOURCE-002");

    let valid = minimal(&format!(
        r#"
sources:
  - name: foo
    kind: archive
    digest: "sha512:{SHA512}"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#
    ));
    plan_str(&valid).expect("valid sha512 is accepted");
}

#[test]
fn duplicate_output_across_cells_is_rejected() {
    let yaml = minimal(
        r#"
matrix:
  axes:
    python:
      - value: "3.10"
      - value: "3.11"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-PLAN-002");
}

#[test]
fn release_suffix_is_not_applied_twice() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: "1.${cell.slug}"
  summary: x
  license: MIT
matrix:
  axes:
    python:
      - value: "3.10"
        slug: py310
outputs:
  - name: "foo-${matrix.python}"
    files:
      include: ["/usr/bin/foo"]
"#;
    let plan = plan_str(yaml).expect("valid");
    assert_eq!(plan.cells[0].release, "1.py310");
}

#[test]
fn dist_tag_follows_the_matrix_suffix() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
matrix:
  axes:
    python:
      - value: "3.10"
        slug: py310
outputs:
  - name: "foo-${matrix.python}"
    files:
      include: ["/usr/bin/foo"]
"#;
    let plan = plan_with_policy(yaml, Some("fc44"), None).expect("valid");
    assert_eq!(plan.dist_tag.as_deref(), Some(".fc44"));
    assert_eq!(plan.cells[0].release, "1.py310.fc44");
}

#[test]
fn build_release_is_last_and_canonical() {
    let yaml = minimal(
        r#"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    let plan = plan_with_policy(&yaml, Some(".fc44"), Some(7)).expect("valid");
    assert_eq!(plan.build_release, Some(7));
    assert_eq!(plan.cells[0].release, "1.fc44+build7");
}

#[test]
fn build_release_does_not_require_a_dist_tag() {
    let yaml = minimal(
        r#"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    let plan = plan_with_policy(&yaml, None, Some(0)).expect("valid");
    assert_eq!(plan.cells[0].release, "1+build0");
}

#[test]
fn dist_tag_template_and_dependencies_include_the_suffix() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: "1${profile.dist_tag}"
  summary: x
  license: MIT
outputs:
  - name: foo
    dependencies:
      run: ["bar = ${package.version}-${package.release}"]
    files:
      include: ["/usr/bin/foo"]
"#;
    let plan = plan_with_policy(yaml, Some("fc44"), None).expect("valid");
    assert_eq!(plan.cells[0].release, "1.fc44");
    assert_eq!(plan.cells[0].outputs[0].requires, ["bar = 1.0-1.fc44"]);
}

#[test]
fn template_suffixes_are_reordered_after_the_matrix_suffix() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: "1${profile.dist_tag}+build7"
  summary: x
  license: MIT
matrix:
  axes:
    python:
      - value: "3.10"
        slug: py310
outputs:
  - name: "foo-${matrix.python}"
    files:
      include: ["/usr/bin/foo"]
"#;
    let plan = plan_with_policy(yaml, Some("fc44"), Some(7)).expect("valid");
    assert_eq!(plan.cells[0].release, "1.py310.fc44+build7");
}

#[test]
fn template_suffixes_are_reordered_when_the_build_counter_comes_first() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: "1+build7${profile.dist_tag}"
  summary: x
  license: MIT
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    let plan = plan_with_policy(yaml, Some("fc44"), Some(7)).expect("valid");
    assert_eq!(plan.cells[0].release, "1.fc44+build7");
}

#[test]
fn a_release_ending_in_the_tag_is_not_consumed() {
    let yaml = minimal(
        r#"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    )
    .replace("release: 1", "release: 2.1.1");
    let plan = plan_with_policy(&yaml, Some("1"), None).expect("valid");
    assert_eq!(plan.cells[0].release, "2.1.1");
}

#[test]
fn dependency_scope_gets_automatically_appended_suffixes() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
outputs:
  - name: foo
    dependencies:
      run: ["bar = ${package.version}-${package.release}"]
    files:
      include: ["/usr/bin/foo"]
"#;
    let plan = plan_with_policy(yaml, Some("fc44"), Some(7)).expect("valid");
    assert_eq!(plan.cells[0].release, "1.fc44+build7");
    assert_eq!(
        plan.cells[0].outputs[0].requires,
        ["bar = 1.0-1.fc44+build7"]
    );
}

#[test]
fn an_empty_release_is_not_rescued_by_policy_suffixes() {
    let yaml = minimal(
        r#"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    )
    .replace("release: 1", r#"release: "${profile.dist_tag}""#);
    let error = plan_with_policy(&yaml, Some("fc44"), Some(7)).expect_err("empty base");
    assert_eq!(error.code, "RPMX-PLAN-004");
}

#[test]
fn invalid_dist_tags_are_rejected() {
    let yaml = minimal(
        r#"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    for tag in [
        "",
        " fc44",
        "fc 44",
        "fc/44",
        "..fc44",
        "fc-44",
        "fc44+build1",
        ".",
        "fc44.",
    ] {
        let error = plan_with_policy(&yaml, Some(tag), None).expect_err("invalid tag");
        assert_eq!(error.code, "RPMX-PLAN-012", "tag {tag:?}");
    }
}

#[test]
fn escaped_template_is_literal() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
outputs:
  - name: foo
    summary: "literal $${HOME}"
    files:
      include: ["/usr/bin/foo"]
"#;
    let plan = plan_str(yaml).expect("valid");
    assert_eq!(
        plan.cells[0].outputs[0].summary.as_deref(),
        Some("literal ${HOME}")
    );
}

#[test]
fn duplicate_test_name_is_rejected() {
    let yaml = minimal(
        r#"
tests:
  - name: smoke
    stage: installed
    argv: ["/usr/bin/foo"]
  - name: smoke
    stage: build
    argv: ["true"]
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-TEST-002");
}

#[test]
fn unresolved_template_from_variable_is_rejected() {
    let yaml = r#"
schema: 1
package:
  name: "${vars.base}-x"
  version: "1.0"
  release: 1
  summary: x
  license: MIT
vars:
  base: foo
  alias: "${package.name}"
outputs:
  - name: "${vars.alias}"
    files:
      include: ["/usr/bin/foo"]
"#;
    assert_eq!(error_code(yaml), "RPMX-TEMPLATE-007");
}

#[test]
fn metapackage_without_files_is_allowed() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
outputs:
  - name: foo
    metapackage: true
"#;
    let plan = plan_str(yaml).expect("metapackage is valid");
    assert_eq!(plan.cells[0].outputs[0].name, "foo");
}

#[test]
fn package_name_cannot_escape_the_output_directory() {
    let yaml = r#"
schema: 1
package:
  name: "../evil"
  version: "1.0"
  release: 1
  summary: x
  license: MIT
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    assert_eq!(error_code(yaml), "RPMX-PLAN-011");
}

#[test]
fn boolean_version_is_rejected() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: true
  release: 1
  summary: x
  license: MIT
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    assert_eq!(error_code(yaml), "RPMX-YAML-001");
}

#[test]
fn go_bump_example_plans_builtin_steps_without_tool_locators() {
    let yaml = include_str!("../../../examples/go-bump/package.yaml");
    let plan = plan_str(yaml).expect("go-bump plans");
    let source = plan.source_assembly.expect("source assembly is present");

    assert!(source.tools.is_empty(), "the recipe must not name an OCI");
    assert_eq!(source.steps.len(), 2);

    let bump = &source.steps[0];
    assert_eq!(bump.uses, "go/bump");
    assert_eq!(bump.command.as_deref(), Some("sh"));
    assert!(bump.args[1].contains("omnibump --deps deps.yaml --tidy"));
    assert!(
        bump.tool_digest.is_none(),
        "tool is resolved by the builder"
    );
    let input = bump.input.as_deref().expect("deps rendered");
    assert!(input.contains("- name: golang.org/x/net"));
    assert!(input.contains("version: v0.33.0"));
    assert_eq!(bump.input_file.as_deref(), Some("deps.yaml"));

    let vendor = &source.steps[1];
    assert_eq!(vendor.uses, "go/vendor");
    assert_eq!(vendor.command.as_deref(), Some("sh"));
    assert!(vendor.args[1].contains("go mod vendor"));
}

#[test]
fn go_acquire_config_lives_on_the_locator() {
    let recipe = parse_recipe(include_str!("../../../examples/go-bump/package.yaml"))
        .expect("go-bump parses");
    let locator = &recipe.sources[0].locators[0];
    assert_eq!(locator.proxy.as_deref(), Some("https://proxy.golang.org"));
    assert_eq!(locator.sumdb.as_deref(), Some("sum.golang.org"));
}

#[test]
fn hello_go_example_plans_go_vendor() {
    let yaml = include_str!("../../../examples/hello-go/package.yaml");
    let plan = plan_str(yaml).expect("hello-go plans");
    let source = plan.source_assembly.expect("source assembly is present");

    assert_eq!(source.steps.len(), 1);
    assert_eq!(source.steps[0].uses, "go/vendor");
}

#[test]
fn go_bump_requires_a_following_go_vendor() {
    let yaml = minimal(
        r#"
source-assembly:
  pipeline:
    - uses: go/bump
      with:
        deps:
          - module: golang.org/x/net
            version: v0.33.0
            reason: CVE-2024-45337
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-ASSEMBLY-016");
}

#[test]
fn unknown_source_step_is_rejected() {
    let yaml = minimal(
        r#"
source-assembly:
  pipeline:
    - uses: go/whatever
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-STEP-011");
}

#[test]
fn go_bump_requires_deps() {
    let yaml = minimal(
        r#"
source-assembly:
  pipeline:
    - uses: go/bump
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-STEP-015");
}

#[test]
fn go_bump_dep_needs_a_reason() {
    let yaml = minimal(
        r#"
source-assembly:
  pipeline:
    - uses: go/bump
      with:
        deps:
          - module: golang.org/x/net
            version: v0.33.0
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-ASSEMBLY-015");
}

#[test]
fn go_bump_rejects_latest() {
    let yaml = minimal(
        r#"
source-assembly:
  pipeline:
    - uses: go/bump
      with:
        deps:
          - module: golang.org/x/net
            version: "@latest"
            reason: CVE-2024-45337
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-ASSEMBLY-014");
}

#[test]
fn go_bump_rejects_duplicate_modules() {
    let yaml = minimal(
        r#"
source-assembly:
  pipeline:
    - uses: go/bump
      with:
        deps:
          - module: golang.org/x/net
            version: v0.33.0
            reason: one
          - module: golang.org/x/net
            version: v0.34.0
            reason: two
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-ASSEMBLY-013");
}

#[test]
fn source_assembly_cannot_use_matrix_templates() {
    let yaml = r#"
schema: 1
package:
  name: foo
  version: "1.0"
  release: 1
  summary: x
  license: MIT
matrix:
  axes:
    python:
      - value: "3.10"
source-assembly:
  pipeline:
    - argv: ["/usr/bin/python${matrix.python}", "-c", "print(1)"]
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#;
    assert_eq!(error_code(yaml), "RPMX-TEMPLATE-005");
}

#[test]
fn source_tools_are_an_explicit_override() {
    let yaml = minimal(&format!(
        r#"
source-assembly:
  tools:
    - name: omnibump
      version: v0.3.0
      digest: "sha512:{SHA512}"
      entrypoint: /usr/bin/omnibump
      locators:
        - kind: oci
          ref: a
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#
    ));
    let plan = plan_str(&yaml).expect("tool override plans");
    let source = plan.source_assembly.expect("source assembly");
    assert_eq!(source.steps.len(), 0);
    assert_eq!(source.tools[0].name, "omnibump");
    assert_eq!(
        source.tools[0].entrypoint.as_deref(),
        Some("/usr/bin/omnibump")
    );
}

#[test]
fn tool_digest_must_be_sha512() {
    let yaml = minimal(
        r#"
source-assembly:
  tools:
    - name: omnibump
      version: v0.3.0
      digest: "md5:deadbeef"
      locators:
        - kind: oci
          ref: a
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    assert_eq!(error_code(&yaml), "RPMX-TOOL-001");
}

#[test]
fn duplicate_tool_name_is_rejected() {
    let yaml = minimal(&format!(
        r#"
source-assembly:
  tools:
    - name: omnibump
      version: v0.3.0
      digest: "sha512:{SHA512}"
      locators:
        - kind: oci
          ref: a
    - name: omnibump
      version: v0.3.1
      digest: "sha512:{SHA512}"
      locators:
        - kind: oci
          ref: b
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#
    ));
    assert_eq!(error_code(&yaml), "RPMX-TOOL-003");
}

#[test]
fn other_builtin_vendor_steps_resolve() {
    let yaml = minimal(
        r#"
source-assembly:
  pipeline:
    - uses: cargo/vendor
    - uses: npm/ci
outputs:
  - name: foo
    files:
      include: ["/usr/bin/foo"]
"#,
    );
    let plan = plan_str(&yaml).expect("builtins plan");
    let source = plan.source_assembly.expect("source assembly");
    assert_eq!(source.steps[0].command.as_deref(), Some("sh"));
    assert!(source.steps[0].args[1].contains("cargo vendor"));
    assert_eq!(source.steps[1].command.as_deref(), Some("sh"));
    assert!(source.steps[1].args[1].contains("npm ci --offline"));
}

#[test]
fn build_tools_are_expanded_and_carried_into_the_plan() {
    let yaml = minimal(
        r#"
build:
  dependencies:
    tools:
      - "${package.name}-tool"
      - "make >= 4"
outputs:
  - name: foo
    files:
      include: ["/usr/bin/**"]
"#,
    );
    let plan = plan_str(&yaml).expect("plan");
    assert_eq!(plan.cells[0].tools, ["foo-tool", "make >= 4"]);
}
