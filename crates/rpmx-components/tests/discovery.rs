use std::fs;
use std::path::Path;

use rpmx_base::error::codes;
use rpmx_components::{discover, Inventory};
use tempfile::tempdir;

fn recipe(root: &Path) -> rpmx_language::Plan {
    let path = root.join("package.yaml");
    fs::write(
        &path,
        r#"schema: 1
package:
  name: app
  version: "1.0"
  release: 1
  summary: inventory fixture
  license: MIT
  source-date-epoch: 1700000000
outputs:
  - name: app
    files:
      include: ["/usr/bin/app"]
"#,
    )
    .expect("recipe");
    let recipe = rpmx_language::load_recipe(&path).expect("load recipe");
    rpmx_language::plan(&recipe, "x86_64").expect("plan")
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, text).expect("write fixture");
}

#[test]
fn discovers_go_cargo_node_and_manual_components() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();

    write(
        root,
        "vendor/modules.txt",
        "# github.com/spf13/cobra v1.8.1\n## explicit; go 1.21\n# golang.org/x/sys v0.0.0-20240101000000-abcdef123456\n# example.org/old v1.0.0 => example.org/new v1.1.0\n# example.org/local v0.5.0 => ./local\n# example.org/none => ./local\n",
    );
    write(
        root,
        "Cargo.lock",
        r#"version = 4

[[package]]
name = "app"
version = "1.0.0"

[[package]]
name = "serde"
version = "1.0.203"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "serde"
version = "0.9.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "blazesym"
version = "0.2.0-rc.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "smithay"
version = "0.7.0"
source = "git+https://github.com/Smithay/smithay.git#20d2dacd"
"#,
    );
    write(
        root,
        "package-lock.json",
        r#"{
  "lockfileVersion": 3,
  "packages": {
    "": {"name": "web", "version": "1.0.0"},
    "node_modules/left-pad": {"version": "1.3.0"},
    "node_modules/left-pad/node_modules/ms": {"version": "2.1.3"},
    "node_modules/optional-missing": {"version": "9.9.9", "optional": true},
    "node_modules/dev-only": {"version": "9.9.9", "dev": true}
  }
}"#,
    );
    write(
        root,
        "node_modules/left-pad/package.json",
        r#"{"name":"left-pad","version":"1.3.0"}"#,
    );
    write(
        root,
        "node_modules/left-pad/node_modules/ms/package.json",
        r#"{"name":"ms","version":"2.1.3"}"#,
    );
    write(
        root,
        "node_modules/dev-only/package.json",
        r#"{"name":"dev-only","version":"9.9.9"}"#,
    );
    write(
        root,
        ".rpmx/bundled/app/zlib.list",
        "zlib=1.3.1\n# comment\n",
    );

    let inventory = discover(root, &cells).expect("discover");
    let provides = inventory.provides_for(&plan.cells[0].outputs[0]);

    assert!(
        provides.contains(&"bundled(golang(github.com/spf13/cobra)) = 1.8.1".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(golang(golang.org/x/sys)) = abcdef1".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(golang(example.org/new)) = 1.1.0".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(golang(example.org/local)) = 0.5.0".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(golang(example.org/none))".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(crate(serde)) = 1.0.203".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(crate(serde)) = 0.9.0".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(crate(blazesym)) = 0.2.0~rc.0".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(crate(smithay)) = 0.7.0+git20d2dacd".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(nodejs-left-pad) = 1.3.0".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(nodejs-ms) = 2.1.3".to_string()),
        "{provides:?}"
    );
    assert!(
        !provides.iter().any(|provide| provide.contains("dev-only")),
        "{provides:?}"
    );
    assert!(
        !provides
            .iter()
            .any(|provide| provide.contains("optional-missing")),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(zlib) = 1.3.1".to_string()),
        "{provides:?}"
    );
}

#[test]
fn a_lockfile_without_a_vendor_tree_is_declared_not_provided() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();

    write(
        root,
        "Cargo.lock",
        r#"version = 4

[[package]]
name = "serde"
version = "1.0.203"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    );
    write(
        root,
        "package-lock.json",
        r#"{"lockfileVersion":3,"packages":{"node_modules/left-pad":{"version":"1.3.0"}}}"#,
    );

    let inventory = discover(root, &cells).expect("discover");
    assert!(inventory.provides_for(&plan.cells[0].outputs[0]).is_empty());
}

#[test]
fn manual_versions_must_agree() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();

    write(root, ".rpmx/bundled/app/one.list", "zlib=1.3.1\n");
    write(root, ".rpmx/bundled/app/two.list", "zlib=1.3.2\n");

    let error = discover(root, &cells).expect_err("conflict");
    assert_eq!(error.code, codes::BUNDLED_002);
}

#[test]
fn an_unknown_output_is_rejected() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();

    write(root, ".rpmx/bundled/nope/zlib.list", "zlib=1.3.1\n");

    let error = discover(root, &cells).expect_err("unknown output");
    assert_eq!(error.code, codes::BUNDLED_003);
}

#[test]
fn the_inventory_round_trips_and_is_deterministic() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();

    write(root, ".rpmx/bundled/app/zlib.list", "zlib=1.3.1\n");
    let inventory = discover(root, &cells).expect("discover");
    let first = inventory.to_json().expect("json");
    let second = inventory.to_json().expect("json");
    assert_eq!(first, second);
    let loaded = Inventory::from_json(&first).expect("parse");
    assert_eq!(loaded.to_json().expect("json"), first);
}

#[test]
fn recipe_declarations_and_output_directories_are_exposed() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let path = root.join("package.yaml");
    fs::write(
        &path,
        r#"schema: 1
package:
  name: app
  version: "1.0"
  release: 1
  summary: recipe declaration fixture
  license: MIT
  source-date-epoch: 1700000000
outputs:
  - name: app
    files:
      include: ["/usr/bin/app"]
    bundled:
      - name: zlib
        version: "1.3.1"
"#,
    )
    .expect("recipe");
    let recipe = rpmx_language::load_recipe(&path).expect("load");
    let plan = rpmx_language::plan(&recipe, "x86_64").expect("plan");
    let cells = plan.cells.iter().collect::<Vec<_>>();

    let inventory = Inventory::from_plan(&plan);
    assert_eq!(
        inventory.provides_for(&plan.cells[0].outputs[0]),
        vec!["bundled(zlib) = 1.3.1"]
    );

    rpmx_components::prepare_output_dirs(root, &cells).expect("dirs");
    assert!(root.join(".rpmx/bundled/app").is_dir());
}

#[cfg(unix)]
#[test]
fn a_symlinked_declaration_root_is_rejected() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();
    let outside = directory.path().join("outside");
    fs::create_dir_all(&outside).expect("outside");
    fs::create_dir_all(root.join(".rpmx")).expect(".rpmx");
    symlink(&outside, root.join(".rpmx/bundled")).expect("symlink");

    let error = rpmx_components::prepare_output_dirs(root, &cells).expect_err("symlink");
    assert_eq!(error.code, codes::BUNDLED_003);
}

#[test]
fn control_characters_in_a_manual_version_are_rejected() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();

    write(root, ".rpmx/bundled/app/zlib.list", "zlib=1.3.1\u{1}\n");

    let error = discover(root, &cells).expect_err("control character");
    assert_eq!(error.code, codes::BUNDLED_003);
}

#[test]
fn discovers_yarn_classic_and_berry() {
    let classic = tempdir().expect("classic");
    let root = classic.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();
    write(
        root,
        "node_modules/left-pad/package.json",
        r#"{"name":"left-pad","version":"1.3.0"}"#,
    );
    write(
        root,
        "node_modules/dev-only/package.json",
        r#"{"name":"dev-only","version":"9.9.9"}"#,
    );
    write(
        root,
        "package.json",
        r#"{"dependencies":{"left-pad":"^1.0.0"},"devDependencies":{"dev-only":"^1.0.0"}}"#,
    );
    write(
        root,
        "yarn.lock",
        "# yarn lockfile v1\n\nleft-pad@^1.0.0:\n  version \"1.3.0\"\n  resolved \"https://example.org/left-pad.tgz\"\n\n\"local@file:bar\":\n  version \"0.0.0\"\n\n\"dev-only@^1.0.0\":\n  version \"9.9.9\"\n",
    );
    let inventory = discover(root, &cells).expect("classic");
    let provides = inventory.provides_for(&plan.cells[0].outputs[0]);
    assert!(provides.contains(&"bundled(nodejs-left-pad) = 1.3.0".to_string()));
    assert!(
        !provides.iter().any(|provide| provide.contains("local")),
        "{provides:?}"
    );
    assert!(
        !provides.iter().any(|provide| provide.contains("dev-only")),
        "{provides:?}"
    );

    let berry = tempdir().expect("berry");
    let root = berry.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();
    write(root, ".yarn/cache/.keep", "");
    write(
        root,
        "package.json",
        r#"{"dependencies":{"@scope/name":"^1.0.0","left-pad-alias":"npm:left-pad@^1.3.0"}}"#,
    );
    write(root, ".yarn/cache/@scope-name-npm-1.2.3-abcdef.zip", "");
    write(root, ".yarn/cache/left-pad-npm-1.3.0-abcdef.zip", "");
    write(
        root,
        "yarn.lock",
        r#"__metadata:
  version: 8
  cacheKey: 10c0

"@scope/name@npm:^1.0.0":
  version: 1.2.3
  resolution: "@scope/name@npm:1.2.3"

"left-pad-alias@npm:left-pad@^1.3.0":
  version: 1.3.0
  resolution: "left-pad@npm:1.3.0"

"local@workspace:.":
  version: 0.0.0-use.local
  resolution: "local@workspace:."
"#,
    );
    let inventory = discover(root, &cells).expect("berry");
    let provides = inventory.provides_for(&plan.cells[0].outputs[0]);
    assert!(
        provides.contains(&"bundled(nodejs-@scope/name) = 1.2.3".to_string()),
        "{provides:?}"
    );
    assert!(
        !provides.iter().any(|provide| provide.contains("local")),
        "{provides:?}"
    );
    assert!(
        !provides.iter().any(|provide| provide.contains("dev-only")),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(nodejs-left-pad) = 1.3.0".to_string()),
        "{provides:?}"
    );
    assert!(
        !provides
            .iter()
            .any(|provide| provide.contains("left-pad-alias")),
        "{provides:?}"
    );
}

#[test]
fn discovers_pnpm_v6_and_v9() {
    let v9 = tempdir().expect("v9");
    let root = v9.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();
    write(root, "node_modules/.keep", "");
    write(
        root,
        "node_modules/left-pad/package.json",
        r#"{"name":"left-pad","version":"1.3.0"}"#,
    );
    write(
        root,
        "node_modules/@scope/name/package.json",
        r#"{"name":"@scope/name","version":"1.2.3"}"#,
    );
    write(
        root,
        "node_modules/.pnpm/eslint@9.0.0/node_modules/eslint/package.json",
        r#"{"name":"eslint","version":"9.0.0"}"#,
    );
    write(
        root,
        "pnpm-lock.yaml",
        r#"lockfileVersion: '9.0'

importers:
  .:
    dependencies:
      left-pad:
        specifier: ^1.0.0
        version: 1.3.0
      '@scope/name':
        specifier: ^1.0.0
        version: 1.2.3
    devDependencies:
      eslint:
        specifier: ^9.0.0
        version: 9.0.0

packages:
  left-pad@1.3.0:
    resolution: {integrity: sha512-example}
  '@scope/name@1.2.3':
    resolution: {integrity: sha512-example}
  eslint@9.0.0:
    resolution: {integrity: sha512-example}

snapshots:
  left-pad@1.3.0: {}
  '@scope/name@1.2.3': {}
  eslint@9.0.0: {}
"#,
    );
    let inventory = discover(root, &cells).expect("v9");
    let provides = inventory.provides_for(&plan.cells[0].outputs[0]);
    assert!(
        provides.contains(&"bundled(nodejs-left-pad) = 1.3.0".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(nodejs-@scope/name) = 1.2.3".to_string()),
        "{provides:?}"
    );
    assert!(
        !provides.iter().any(|provide| provide.contains("eslint")),
        "{provides:?}"
    );

    let v6 = tempdir().expect("v6");
    let root = v6.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();
    write(root, "node_modules/.keep", "");
    write(
        root,
        "node_modules/left-pad/package.json",
        r#"{"name":"left-pad","version":"1.3.0"}"#,
    );
    write(
        root,
        "node_modules/@scope/name/package.json",
        r#"{"name":"@scope/name","version":"1.2.3"}"#,
    );
    write(
        root,
        "pnpm-lock.yaml",
        r#"lockfileVersion: 5.4

packages:
  /left-pad/1.3.0:
    resolution: {integrity: sha512-example}
  /@scope/name/1.2.3:
    resolution: {integrity: sha512-example}
"#,
    );
    let inventory = discover(root, &cells).expect("v6");
    let provides = inventory.provides_for(&plan.cells[0].outputs[0]);
    assert!(
        provides.contains(&"bundled(nodejs-left-pad) = 1.3.0".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(nodejs-@scope/name) = 1.2.3".to_string()),
        "{provides:?}"
    );
}

#[test]
fn pnpm_merges_documents_and_prefers_entry_identity() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();
    write(root, "node_modules/.keep", "");
    write(
        root,
        "node_modules/left-pad/package.json",
        r#"{"name":"left-pad","version":"1.3.0"}"#,
    );
    write(
        root,
        "node_modules/is-negative/package.json",
        r#"{"name":"is-negative","version":"2.1.0"}"#,
    );
    write(
        root,
        "pnpm-lock.yaml",
        r#"lockfileVersion: '9.0'
---
packages:
  is-negative@https://codeload.github.com/kevva/is-negative/tar.gz/abc:
    name: is-negative
    version: 2.1.0
  /left-pad@1.3.0:
    resolution: {integrity: sha512-example}
"#,
    );
    let inventory = discover(root, &cells).expect("pnpm");
    let provides = inventory.provides_for(&plan.cells[0].outputs[0]);
    assert!(
        provides.contains(&"bundled(nodejs-is-negative) = 2.1.0".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(nodejs-left-pad) = 1.3.0".to_string()),
        "{provides:?}"
    );
}

#[test]
fn yarn_classic_finds_nested_installed_versions() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();
    write(
        root,
        "node_modules/ms/package.json",
        r#"{"name":"ms","version":"2.1.3"}"#,
    );
    write(
        root,
        "node_modules/parent/package.json",
        r#"{"name":"parent","version":"1.0.0"}"#,
    );
    write(
        root,
        "node_modules/parent/node_modules/ms/package.json",
        r#"{"name":"ms","version":"0.7.3"}"#,
    );
    write(root, "package.json", r#"{"dependencies":{"ms":"^2.0.0"}}"#);
    write(
        root,
        "yarn.lock",
        "# yarn lockfile v1\n\nms@^2.0.0:\n  version \"2.1.3\"\n\nms@^0.7.0:\n  version \"0.7.3\"\n",
    );
    let inventory = discover(root, &cells).expect("yarn");
    let provides = inventory.provides_for(&plan.cells[0].outputs[0]);
    assert!(
        provides.contains(&"bundled(nodejs-ms) = 2.1.3".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(nodejs-ms) = 0.7.3".to_string()),
        "{provides:?}"
    );
}

#[test]
fn discovers_python_ruby_and_php_components() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let plan = recipe(root);
    let cells = plan.cells.iter().collect::<Vec<_>>();

    write(
        root,
        "site-packages/requests-2.32.5.dist-info/METADATA",
        "Metadata-Version: 2.1\nName: requests\nVersion: 2.32.5\n",
    );
    write(
        root,
        "_vendor/vendored.txt",
        "urllib3==1.26.20\ncertifi==2026.1.4\n",
    );
    write(
        root,
        "site-packages/app-1.0.dist-info/METADATA",
        "Metadata-Version: 2.1\nName: app\nVersion: 1.0\n",
    );
    write(
        root,
        "Gemfile.lock",
        "GEM\n  remote: https://rubygems.org/\n  specs:\n    rake (13.3.1)\n\nDEPENDENCIES\n  rake\n",
    );
    write(
        root,
        "vendor/bundle/ruby/3.3.0/specifications/rake-13.3.1.gemspec",
        "s.name = \"rake\".freeze\ns.version = \"13.3.1\"\n",
    );
    write(
        root,
        "composer.lock",
        r#"{"packages":[{"name":"psr/container","version":"1.1.1"},{"name":"dev/tool","version":"9.9.9","dev":true}],"packages-dev":[]}"#,
    );
    write(root, "vendor/autoload.php", "");
    write(
        root,
        "vendor/composer/installed.json",
        r#"{"packages":[{"name":"psr/container","version":"1.1.1"},{"name":"dev/tool","version":"9.9.9"}],"dev-package-names":["dev/tool"]}"#,
    );

    let inventory = discover(root, &cells).expect("components");
    let provides = inventory.provides_for(&plan.cells[0].outputs[0]);
    assert!(
        provides.contains(&"bundled(python3dist(requests)) = 2.32.5".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(python3dist(urllib3)) = 1.26.20".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(python3dist(certifi)) = 2026.1.4".to_string()),
        "{provides:?}"
    );
    assert!(
        !provides
            .iter()
            .any(|provide| provide.contains("python3dist(app)")),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(rubygem-rake) = 13.3.1".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(php-composer(psr/container)) = 1.1.1".to_string()),
        "{provides:?}"
    );
    assert!(
        !provides.iter().any(|provide| provide.contains("dev/tool")),
        "{provides:?}"
    );
}

#[test]
fn python_self_metadata_with_rpm_prefix_is_skipped() {
    let directory = tempdir().expect("tempdir");
    let root = directory.path();
    let path = root.join("package.yaml");
    fs::write(
        &path,
        r#"schema: 1
package:
  name: python3-requests
  version: "1.0"
  release: 1
  summary: python self fixture
  license: MIT
  source-date-epoch: 1700000000
outputs:
  - name: python3-requests
    files:
      include: ["/usr/lib/python3/site-packages/**"]
"#,
    )
    .expect("recipe");
    let recipe = rpmx_language::load_recipe(&path).expect("load");
    let plan = rpmx_language::plan(&recipe, "x86_64").expect("plan");
    let cells = plan.cells.iter().collect::<Vec<_>>();
    write(
        root,
        "site-packages/requests-2.32.5.dist-info/METADATA",
        "Metadata-Version: 2.1\nName: requests\nVersion: 2.32.5\n",
    );
    let inventory = discover(root, &cells).expect("discover");
    assert!(
        !inventory
            .provides_for(&plan.cells[0].outputs[0])
            .iter()
            .any(|provide| provide.contains("python3dist(requests)")),
        "{:?}",
        inventory.provides_for(&plan.cells[0].outputs[0])
    );
}
