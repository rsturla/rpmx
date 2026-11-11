use std::process::{Command, Output};

const RECIPE: &str = "schema: 1\npackage:\n  name: probe\n  version: '1.0'\n  release: 1\n  summary: probe\n  license: MIT\n  source-date-epoch: 1700000000\nbuild:\n  pipeline:\n    - runs: 'true'\noutputs:\n  - name: probe\n    metapackage: true\n";
const METADATA: &str = "metadata:\n  eternal:\n    update:\n      version: '1.0' # downstream value\n      url: https://example.invalid/1.0\n      template: '${version}'\n";

fn rpmx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .expect("run rpmx")
}

#[test]
fn recipe_metadata_survives_bump_and_participates_in_lock_checks() {
    for included in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("package.yaml");
        let metadata_file = if included {
            std::fs::write(&root, format!("{RECIPE}include: [metadata.yaml]\n")).unwrap();
            let fragment = directory.path().join("metadata.yaml");
            std::fs::write(&fragment, METADATA).unwrap();
            fragment
        } else {
            std::fs::write(&root, format!("{METADATA}{RECIPE}")).unwrap();
            root.clone()
        };
        let file = root.to_str().unwrap();
        for command in ["lint", "plan"] {
            let output = rpmx(&[command, file]);
            assert!(output.status.success(), "{command}: {output:?}");
        }
        let output = rpmx(&["bump", "1.1", file, "--no-fetch"]);
        assert!(output.status.success(), "{output:?}");
        let text = std::fs::read_to_string(&metadata_file).unwrap();
        assert!(text.starts_with(METADATA), "metadata changed: {text}");
        assert_eq!(
            rpmx_core::load_recipe(&root).unwrap().package.version,
            "1.1"
        );
        let output = rpmx(&["lock", file]);
        assert!(output.status.success(), "{output:?}");
        let output = rpmx(&["lock", file, "--check"]);
        assert!(output.status.success(), "{output:?}");
        std::fs::write(
            &metadata_file,
            text.replace("downstream value", "changed metadata"),
        )
        .unwrap();
        let output = rpmx(&["lock", file, "--check"]);
        assert_eq!(output.status.code(), Some(3), "{output:?}");
    }
}
