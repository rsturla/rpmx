use rpmx_language::{load_recipe_bundle, parse_recipe, plan};

const RECIPE: &str = "schema: 1\npackage:\n  name: probe\n  version: '1.0'\n  release: 1\n  summary: probe\n  license: MIT\nbuild:\n  pipeline:\n    - runs: 'true'\noutputs:\n  - name: probe\n    metapackage: true\n";
const METADATA: &str = r#"metadata:
  eternal:
    update:
      enabled: true
      attempts: 3
      template: '${version}-${package.version}'
      extra: [null, false, 1.5, {name: value}]
  another-tool: arbitrary
"#;

#[test]
fn opaque_metadata_round_trips_without_changing_the_plan() {
    let plain = parse_recipe(RECIPE).expect("recipe");
    assert!(plain.metadata.is_empty());
    assert!(!serde_yaml_ng::to_string(&plain)
        .unwrap()
        .contains("metadata:"));
    let recipe = parse_recipe(&format!("{RECIPE}{METADATA}")).expect("metadata");
    let serialized = serde_yaml_ng::to_string(&recipe).expect("serialize");
    let restored = parse_recipe(&serialized).expect("round trip");
    assert_eq!(restored.metadata, recipe.metadata);
    assert_eq!(
        recipe.metadata["eternal"]["update"]["template"],
        "${version}-${package.version}"
    );
    assert!(recipe.vars.is_empty());
    assert_eq!(
        serde_json::to_value(plan(&recipe, "x86_64").unwrap()).unwrap(),
        serde_json::to_value(plan(&plain, "x86_64").unwrap()).unwrap()
    );
    let attempted_variable = format!("{RECIPE}{METADATA}").replace(
        "runs: 'true'",
        "runs: 'echo ${metadata.eternal.update.attempts}'",
    );
    assert!(plan(&parse_recipe(&attempted_variable).unwrap(), "x86_64").is_err());
}

#[test]
fn metadata_requires_a_map_and_other_fields_remain_strict() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("package.yaml");
    for value in ["null", "false", "3", "text", "[]"] {
        let text = format!("{RECIPE}metadata: {value}\n");
        assert!(parse_recipe(&text).is_err(), "accepted {value}");
        std::fs::write(&path, text).unwrap();
        assert!(
            load_recipe_bundle(&path).is_err(),
            "bundle accepted {value}"
        );
    }
    assert!(parse_recipe(&format!("{RECIPE}unrecognized: true\n")).is_err());
    assert!(parse_recipe(&RECIPE.replace("pipeline:", "unrecognized:")).is_err());
}

#[test]
fn included_metadata_has_one_owner_and_does_not_inherit_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("package.yaml");
    let fragment = directory.path().join("metadata.yaml");
    std::fs::write(&root, format!("{RECIPE}include: [metadata.yaml]\n")).unwrap();
    std::fs::write(&fragment, METADATA).unwrap();
    std::fs::write(directory.path().join("rpmx-defaults.yaml"),
        "schema: 1\npackage: {}\nmetadata:\n  defaults-only: true\n  eternal:\n    update:\n      inherited: true\n").unwrap();
    let bundle = load_recipe_bundle(&root).expect("bundle");
    assert_eq!(
        bundle.owner("metadata"),
        Some(fragment.canonicalize().unwrap().as_path())
    );
    assert_eq!(
        bundle.recipe.metadata,
        parse_recipe(&format!("{RECIPE}{METADATA}"))
            .unwrap()
            .metadata
    );
    assert!(bundle
        .files
        .iter()
        .any(|file| file.name == "rpmx-defaults.yaml"));
    for duplicate in ["metadata: {}\n", METADATA] {
        std::fs::write(
            &root,
            format!("{RECIPE}include: [metadata.yaml]\n{duplicate}"),
        )
        .unwrap();
        let error = load_recipe_bundle(&root).expect_err("duplicate metadata");
        assert!(error.message.contains("metadata.yaml"), "{error:?}");
        assert!(error.message.contains("package.yaml"), "{error:?}");
        assert_eq!(error.code, rpmx_base::error::codes::INCLUDE_002);
    }
}
