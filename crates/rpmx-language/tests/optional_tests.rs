use rpmx_language::{load_recipe_bundle, parse_recipe, plan};

const RECIPE: &str = r#"schema: 2
package:
  name: fixture
  version: '1'
  release: 1
  summary: optional selection fixture
  license: MIT
build:
  tests:
    - name: required-source
      script: 'true'
    - name: upstream
      optional: true
      script: 'true'
outputs:
  - name: fixture
    metapackage: true
    tests:
      - name: required-installed
        script: 'true'
      - name: optional-installed
        optional: true
        script: 'true'
"#;

#[test]
fn optional_metadata_is_explicit_in_source_and_installed_plans_and_recipe_binding() {
    let recipe = parse_recipe(RECIPE).unwrap();
    let planned = plan(&recipe, "x86_64").unwrap();
    let cell = &planned.cells[0];
    assert!(!cell.build_tests[0].optional);
    assert!(cell.build_tests[1].optional);
    assert!(!cell.tests[0].optional);
    assert!(cell.tests[1].optional);
    let json = serde_json::to_value(&planned).unwrap();
    assert_eq!(json["cells"][0]["build_tests"][1]["optional"], true);
    assert_eq!(json["cells"][0]["tests"][1]["optional"], true);
    let serialized = serde_yaml_ng::to_string(&recipe).unwrap();
    let restored = parse_recipe(&serialized).unwrap();
    assert!(restored.build.tests[1].optional);
    assert!(restored.outputs[0].tests[1].optional);

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("package.yaml");
    std::fs::write(&path, RECIPE).unwrap();
    let before = load_recipe_bundle(&path).unwrap().digest().to_owned();
    std::fs::write(&path, RECIPE.replace("optional: true", "optional: false")).unwrap();
    assert_ne!(before, load_recipe_bundle(&path).unwrap().digest());
}

#[test]
fn optional_is_a_boolean_not_a_suite_name_or_expression() {
    for value in ["upstream", "1", "[]"] {
        assert!(
            parse_recipe(&RECIPE.replace("optional: true", &format!("optional: {value}"))).is_err()
        );
    }
}
