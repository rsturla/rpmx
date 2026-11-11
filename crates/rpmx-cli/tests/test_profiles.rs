use std::path::Path;
use std::process::{Command, Output};

fn rpmx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .unwrap()
}

fn succeeds(args: &[&str]) -> Output {
    let output = rpmx(args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    output
}

#[test]
fn test_profiles_conflict_with_pinned_roots_and_disabled_tests() {
    for args in [
        vec!["test", "--test-profile", "runtime.yaml", "--test-root", "/"],
        vec![
            "test",
            "--test-profile",
            "runtime.yaml",
            "--test-root-lock",
            "root.yaml",
        ],
        vec!["build", "--test-profile", "runtime.yaml", "--no-tests"],
        vec![
            "rebuild",
            "source.src.rpm",
            "--test-profile",
            "runtime.yaml",
            "--no-tests",
        ],
    ] {
        let output = rpmx(&args);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    }
}

fn profile(lock: &rpmx_core::root::RootLock, seed: Vec<String>) -> rpmx_core::profile::Profile {
    rpmx_core::profile::Profile {
        document: "rpmx-profile".into(),
        version: 1,
        name: Some("profile-test".into()),
        releasever: lock.releasever.clone(),
        dist_tag: lock.dist_tag.clone(),
        archs: lock.archs.clone(),
        builder: rpmx_core::profile::Builder {
            image: lock.builder.as_ref().map(|builder| builder.image.clone()),
        },
        repositories: lock.repositories.clone(),
        packages: rpmx_core::profile::Packages {
            install: seed,
            reinstall: Vec::new(),
        },
        cross: None,
        modules: Default::default(),
        options: lock.options.clone(),
        paths: rpmx_core::profile::Paths {
            prefix: lock.prefix.clone(),
            libdir: lock.libdir.clone(),
            libdir_by_arch: Default::default(),
        },
        sbom: false,
    }
}

#[test]
#[ignore = "set RPMX_TEST_RESOLUTION_LOCK to a native root lock with usable repositories and a builder image"]
fn source_builds_and_standalone_tests_select_independent_repository_profiles() {
    let source_lock = std::env::var("RPMX_TEST_RESOLUTION_LOCK").expect("root lock");
    let mut lock = rpmx_core::root::load_lock(Path::new(&source_lock)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let dependency_recipe = directory.path().join("dependency.yaml");
    std::fs::write(
        &dependency_recipe,
        r#"schema: 1
package:
  name: profile-runtime
  version: "1"
  release: 1
  summary: Test runtime dependency
  license: MIT
  source-date-epoch: 1704067200
outputs:
  - name: profile-runtime
"#,
    )
    .unwrap();
    let stage = directory.path().join("stage");
    std::fs::create_dir_all(stage.join("usr/share/profile-runtime")).unwrap();
    std::fs::write(stage.join("usr/share/profile-runtime/value"), "runtime\n").unwrap();
    let dependency_output = directory.path().join("dependency-output");
    succeeds(&[
        "build",
        dependency_recipe.to_str().unwrap(),
        "--stage",
        stage.to_str().unwrap(),
        "--output",
        dependency_output.to_str().unwrap(),
        "--no-tests",
    ]);
    let published = directory.path().join("published");
    succeeds(&[
        "repo",
        "--rpms",
        dependency_output
            .join(std::env::consts::ARCH)
            .to_str()
            .unwrap(),
        "--output",
        published.to_str().unwrap(),
    ]);
    lock.repositories.push(rpmx_core::profile::Repository {
        id: "runtime-only".into(),
        baseurl: Some(format!("file://{}", published.display())),
        metalink: None,
        mirrorlist: None,
        gpgkey: None,
        repo_gpgcheck: false,
        exclude: vec!["profile-runtime".into()],
        priority: None,
        auth: None,
    });
    lock.closure_digest = rpmx_core::root::closure_digest(
        &lock.releasever,
        &lock.archs,
        &lock.repositories,
        &lock.packages,
    );
    let build_lock = directory.path().join("build.lock.yaml");
    rpmx_core::root::write_lock(&build_lock, &lock).unwrap();
    let build_profile = directory.path().join("build-policy.yaml");
    let mut build_policy = profile(
        &lock,
        vec!["bash".into(), "coreutils".into(), "/bin/sh".into()],
    );
    std::fs::write(
        &build_profile,
        serde_yaml_ng::to_string(&build_policy).unwrap(),
    )
    .unwrap();
    let runtime_profile = directory.path().join("runtime-policy.yaml");
    let mut runtime_policy = profile(&lock, vec!["unavailable-test-profile-seed".into()]);
    runtime_policy
        .repositories
        .last_mut()
        .unwrap()
        .exclude
        .clear();
    // Relative local repositories must resolve against the test profile file.
    runtime_policy.repositories.last_mut().unwrap().baseurl = Some("file://published".into());
    std::fs::write(
        &runtime_profile,
        serde_yaml_ng::to_string(&runtime_policy).unwrap(),
    )
    .unwrap();
    let recipe = directory.path().join("package.yaml");
    std::fs::write(
        &recipe,
        r#"schema: 1
package:
  name: profile-probe
  version: "1"
  release: 1
  summary: Independent test profiles
  license: MIT
  source-date-epoch: 1704067200
build:
  root:
    profile: build-policy.yaml
  pipeline:
    - runs: |
        set -eu
        test ! -e /usr/share/profile-runtime/value
        mkdir -p "$STAGE/usr/share/profile-probe"
        printf 'probe\n' > "$STAGE/usr/share/profile-probe/value"
outputs:
  - name: profile-probe
    dependencies:
      run: [profile-runtime]
    tests:
      - name: runtime
        stage: installed
        script: |
          test "$(cat /usr/share/profile-probe/value)" = probe
          test "$(cat /usr/share/profile-runtime/value)" = runtime
"#,
    )
    .unwrap();
    let built = directory.path().join("built");
    let output = succeeds(&[
        "build",
        recipe.to_str().unwrap(),
        "--test-profile",
        runtime_profile.to_str().unwrap(),
        "--output",
        built.to_str().unwrap(),
        "--no-index",
        "--json",
    ]);
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["ok"], true);
    let binaries = built.join(std::env::consts::ARCH);
    let evidence = std::fs::read_dir(&binaries)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".test-root.lock.json"))
        .unwrap();
    let test_lock = rpmx_core::root::load_lock(&evidence).unwrap();
    assert!(test_lock
        .packages
        .values()
        .flatten()
        .any(|package| package.name == "profile-runtime"));
    assert!(!test_lock
        .packages
        .values()
        .flatten()
        .any(|package| package.name == "unavailable-test-profile-seed"));

    // Standalone tests must not resolve a build root just to read its layout.
    build_policy.packages.install = vec!["unavailable-build-profile-seed".into()];
    std::fs::write(
        &build_profile,
        serde_yaml_ng::to_string(&build_policy).unwrap(),
    )
    .unwrap();
    let reports = directory.path().join("reports");
    let output = succeeds(&[
        "test",
        recipe.to_str().unwrap(),
        "--test-profile",
        runtime_profile.to_str().unwrap(),
        "--install-dir",
        binaries.to_str().unwrap(),
        "--test-output",
        reports.to_str().unwrap(),
        "--json",
    ]);
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["ok"], true);
    let retained =
        rpmx_core::root::load_lock(&reports.join("environments/1/root.lock.yaml")).unwrap();
    assert!(retained
        .packages
        .values()
        .flatten()
        .any(|package| package.name == "profile-runtime"));

    // The same RPM cannot satisfy its runtime dependency under the build policy.
    let excluded = rpmx(&[
        "test",
        recipe.to_str().unwrap(),
        "--root-lock",
        build_lock.to_str().unwrap(),
        "--install-dir",
        binaries.to_str().unwrap(),
        "--test-output",
        directory.path().join("excluded").to_str().unwrap(),
        "--json",
    ]);
    assert!(!excluded.status.success(), "{excluded:?}");
    assert!(String::from_utf8_lossy(&excluded.stdout).contains("profile-runtime"));
}
