use std::path::Path;
use std::process::Command;

fn run(root: &Path, args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .current_dir(root)
        .args(args)
        .output()
        .expect("run rpmx");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn package(root: &Path, name: &str, version: &str, repository: &str, requires: &[&str]) {
    let recipe = root.join(format!("{name}-{version}.yaml"));
    let body = serde_json::json!({
        "schema": 1,
        "package": {"name": name, "version": version, "release": 1,
            "summary": "Transaction fixture", "license": "MIT", "source-date-epoch": 1704067200},
        "outputs": [{"name": name, "arch": "noarch", "metapackage": true,
            "dependencies": {"run": requires}}],
    });
    std::fs::write(&recipe, serde_yaml_ng::to_string(&body).unwrap()).unwrap();
    run(
        root,
        &[
            "build",
            recipe.to_str().unwrap(),
            "--stage",
            "stage",
            "--output",
            repository,
            "--no-tests",
        ],
    );
}

#[test]
#[ignore = "requires rootless podman and the Fedora 44 builder image"]
fn exact_package_requests_and_dependencies_form_one_installable_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("stage")).unwrap();
    package(root, "demo-lib", "2", "published", &[]);
    package(root, "demo-lib", "1", "under-test", &[]);
    package(root, "demo-consumer", "1", "published", &["demo-lib >= 1"]);
    let arch = std::env::consts::ARCH;
    for repository in ["published", "under-test"] {
        let path = root.join(repository).join(arch);
        run(
            root,
            &[
                "repo",
                "--rpms",
                path.to_str().unwrap(),
                "--output",
                path.to_str().unwrap(),
            ],
        );
    }
    let profile = serde_json::json!({
        "document": "rpmx-profile", "version": 1, "name": "transaction-regression",
        "releasever": "44", "dist-tag": ".test", "archs": [arch],
        "builder": {"image": "registry.fedoraproject.org/fedora:44"},
        "repositories": [
            {"id": "published", "baseurl": format!("file://{}", root.join("published").join(arch).display())},
            {"id": "under-test", "baseurl": format!("file://{}", root.join("under-test").join(arch).display()), "priority": 999},
        ],
        "packages": {"install": ["demo-consumer", "demo-lib", "demo-lib-1-1.noarch"]},
        "options": {"weak_deps": false},
    });
    let mut unconstrained = profile.clone();
    unconstrained["packages"]["install"] = serde_json::json!(["demo-consumer", "demo-lib"]);
    std::fs::write(
        root.join("unconstrained.yaml"),
        serde_yaml_ng::to_string(&unconstrained).unwrap(),
    )
    .unwrap();
    run(
        root,
        &[
            "root",
            "resolve",
            "unconstrained.yaml",
            "--output",
            "newest.lock.yaml",
            "--cache",
            "cache",
        ],
    );
    let newest = rpmx_core::root::load_lock(&root.join("newest.lock.yaml")).unwrap();
    let library = newest
        .packages
        .values()
        .flatten()
        .find(|package| package.name == "demo-lib")
        .unwrap();
    assert_eq!(library.evr, "2-1");
    assert_eq!(library.repo_id, "published");

    std::fs::write(
        root.join("profile.yaml"),
        serde_yaml_ng::to_string(&profile).unwrap(),
    )
    .unwrap();
    run(
        root,
        &[
            "root",
            "resolve",
            "profile.yaml",
            "--output",
            "root.lock.yaml",
            "--cache",
            "cache",
        ],
    );
    let lock = rpmx_core::root::load_lock(&root.join("root.lock.yaml")).unwrap();
    let packages: Vec<_> = lock.packages.values().flatten().collect();
    assert_eq!(
        packages.len(),
        2,
        "the higher published version must not join the lock"
    );
    let library = packages
        .iter()
        .find(|package| package.name == "demo-lib")
        .unwrap();
    assert_eq!(library.evr, "1-1");
    assert_eq!(library.repo_id, "under-test");
    run(
        root,
        &[
            "root",
            "assemble",
            "root.lock.yaml",
            "--root",
            "installed",
            "--cache",
            "cache",
            "--output",
            "assembled.lock.yaml",
        ],
    );

    // A genuinely incompatible consumer must fail resolution, rather than
    // producing a lock that contains both versions and fails at assembly.
    package(root, "demo-consumer", "2", "published", &["demo-lib = 2"]);
    let published = root.join("published").join(arch);
    run(
        root,
        &[
            "repo",
            "--rpms",
            published.to_str().unwrap(),
            "--output",
            published.to_str().unwrap(),
        ],
    );
    let mut incompatible = profile.clone();
    incompatible["packages"]["install"] = serde_json::json!([
        "demo-consumer-2-1.noarch",
        "demo-lib",
        "demo-lib-1-1.noarch"
    ]);
    std::fs::write(
        root.join("incompatible.yaml"),
        serde_yaml_ng::to_string(&incompatible).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .current_dir(root)
        .args([
            "root",
            "resolve",
            "incompatible.yaml",
            "--output",
            "unsatisfiable.lock.yaml",
            "--cache",
            "cache",
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "incompatible exact requirements must fail"
    );
    assert!(!root.join("unsatisfiable.lock.yaml").exists());
}
