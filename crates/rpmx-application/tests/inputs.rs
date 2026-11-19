use rpmx_application::*;
use std::process::Command;

#[test]
fn packaging_prebuilt_sources_needs_resolved_inputs_but_no_assembled_root() {
    use rpmx_root::root::*;
    let work = tempfile::tempdir().unwrap();
    let directory = work.path();
    let arch = rpmx_base::arch::native_arch();
    let mut lock: RootLock = serde_json::from_value(serde_json::json!({
        "document": DOCUMENT, "version": VERSION, "profile": "unassembled",
        "archs": [arch], "releasever": "44", "repositories": [], "closure_digest": "",
        "packages": {"noarch": [{"name": "base", "evr": "1-1", "origin": {"kind": "candidate"},
            "checksum": format!("sha512:{}", "0".repeat(128)), "size": 0}]}
    }))
    .unwrap();
    lock.closure_digest = closure_digest(
        &lock.releasever,
        &lock.archs,
        &lock.repositories,
        &lock.packages,
    );
    let path = directory.join("root.lock.yaml");
    write_lock(&path, &lock).unwrap();
    let recipe = directory.join("package.yaml");
    std::fs::write(&recipe, "schema: 2\npackage:\n  name: probe\n  version: '1'\n  release: 1\n  summary: unassembled inputs\n  license: MIT\noutputs:\n  - name: probe\n    metapackage: true\n").unwrap();
    let sources = directory.join("sources");
    std::fs::create_dir(&sources).unwrap();
    let report = Application::new(ApplicationConfig {
        offline: true,
        cache: Some(directory.join("cas")),
        root: BuildRoot::Locked {
            lock: path,
            root: None,
        },
        ..Default::default()
    })
    .execute(ApplicationRequest {
        operation: Operation::Srpm(SrpmInput {
            recipe,
            arch: Some(arch.into()),
            sources: Some(sources),
            cell: None,
            include_optional_tests: false,
        }),
        output: directory.join("prepared"),
    })
    .unwrap();
    assert!(report.success(), "{report:?}");
    assert_eq!(report.artifacts.srpms.len(), 1);
    assert!(!directory.join("roots").exists());
    let path = directory
        .join("prepared/inputs")
        .join(format!("{arch}.json"));
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(original["inputs"]["root"].get("root_digest").is_none());
    assert!(original["inputs"]["root"].get("rpmdb_digest").is_none());
    let mut with_receipt = original["inputs"].clone();
    with_receipt["root"]["root_digest"] = serde_json::json!(format!("sha512:{}", "0".repeat(128)));
    let mut changed_build = original["inputs"].clone();
    changed_build["root"]["prefix"] = serde_json::json!("/opt");
    for (index, field, value) in [
        (0, "version", serde_json::json!(2)),
        (1, "recipe", serde_json::json!("different-recipe")),
        (2, "target", serde_json::json!("different-target")),
        (3, "payloads", serde_json::json!([])),
        (4, "inputs", with_receipt),
        (5, "inputs", changed_build),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        let input_path = directory.join(format!("changed-{index}.json"));
        std::fs::write(&input_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        let mut request = ApplicationRequest::rebuild(
            &report.artifacts.srpms[0],
            directory.join(format!("rejected-{index}")),
        );
        let Operation::Rebuild(input) = &mut request.operation else {
            unreachable!()
        };
        input.prepared_inputs = Some(input_path);
        let rejected = Application::new(ApplicationConfig {
            offline: true,
            cache: Some(directory.join("empty/cas")),
            ..Default::default()
        })
        .execute(request)
        .unwrap();
        assert!(!rejected.success());
        let expected = if index == 5 {
            "RPMX-ROOT-016"
        } else {
            "RPMX-SRPM-006"
        };
        assert_eq!(rejected.primary.unwrap().code, expected);
        assert!(rejected.artifacts.rpms.is_empty());
    }
}

#[test]
#[ignore = "requires rootless Podman, createrepo_c, bubblewrap and the Fedora 44 builder image"]
fn shared_srpm_replays_source_tests_on_a_fresh_offline_worker() {
    let directory = tempfile::tempdir().unwrap();
    let work = directory.path();
    let repo = work.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut bash = rpm::PackageBuilder::new("bash", "1", "MIT", "noarch", "test runtime");
    bash.release("1");
    let mut files = std::collections::BTreeSet::new();
    for command in ["/usr/bin/bash", "/usr/bin/sh", "/usr/bin/mkdir"] {
        let libraries = Command::new("ldd").arg(command).output().unwrap();
        assert!(libraries.status.success());
        files.insert(command.to_string());
        files.extend(
            String::from_utf8(libraries.stdout)
                .unwrap()
                .split_whitespace()
                .filter(|part| part.starts_with('/'))
                .map(str::to_string),
        );
    }
    for file in files {
        bash.with_file(&file, rpm::FileOptions::new(&file)).unwrap();
    }
    bash.build()
        .unwrap()
        .write_file(repo.join("bash.rpm"))
        .unwrap();
    rpm::PackageBuilder::new("coreutils", "1", "MIT", "noarch", "test tools")
        .release("1")
        .build()
        .unwrap()
        .write_file(repo.join("coreutils.rpm"))
        .unwrap();
    let mut tool =
        rpm::PackageBuilder::new("probe-test-tool", "1", "MIT", "noarch", "source-test input");
    tool.release("1")
        .with_file_contents(
            b"ready",
            rpm::FileOptions::new("/usr/share/probe-tool/value"),
        )
        .unwrap();
    tool.build()
        .unwrap()
        .write_file(repo.join("tool.rpm"))
        .unwrap();
    let indexed = Command::new("createrepo_c").arg(&repo).output().unwrap();
    assert!(indexed.status.success(), "{indexed:?}");
    let arch = rpmx_base::arch::native_arch();
    let profile = work.join("profile.yaml");
    std::fs::write(
        &profile,
        format!(
            r#"
document: rpmx-profile
version: 1
archs: [{arch}]
releasever: '44'
builder:
  image: registry.fedoraproject.org/fedora:44
repositories:
  - id: source-worker
    baseurl: file://{}
packages:
  install: [bash, coreutils]
"#,
            repo.display()
        ),
    )
    .unwrap();
    let recipe = work.join("package.yaml");
    std::fs::write(
        &recipe,
        r#"
schema: 2
package:
  name: probe
  version: '1'
  release: 1
  summary: source-test transport probe
  license: MIT
build:
  packages: [bash, coreutils]
  pipeline:
    - runs: test ! -e /usr/share/probe-tool/value
  tests:
    - name: upstream
      packages: [probe-test-tool]
      script: test "$(</usr/share/probe-tool/value)" = ready
outputs:
  - name: probe
    metapackage: true
"#,
    )
    .unwrap();
    let prepared = work.join("prepared");
    let cache = work.join("source-worker/cas");
    let source = Application::new(ApplicationConfig {
        cache: Some(cache.clone()),
        root: BuildRoot::Automatic {
            profile: Some(profile.clone()),
        },
        ..Default::default()
    })
    .execute(ApplicationRequest {
        operation: Operation::Srpm(SrpmInput {
            recipe: recipe.clone(),
            arch: Some("all".into()),
            sources: None,
            cell: None,
            include_optional_tests: false,
        }),
        output: prepared.clone(),
    })
    .unwrap();
    assert!(source.success(), "{source:?}");
    // The local build must produce the same complete, embedded input record.
    let mut request = ApplicationRequest::build(&recipe, work.join("local-build"));
    let Operation::Build(input) = &mut request.operation else {
        unreachable!()
    };
    input.lint = false;
    input.index = false;
    let local = Application::new(ApplicationConfig {
        cache: Some(cache.clone()),
        root: BuildRoot::Automatic {
            profile: Some(profile.clone()),
        },
        ..Default::default()
    })
    .execute(request)
    .unwrap();
    assert!(local.success(), "{local:?}");
    let input_path = prepared.join("inputs").join(format!("{arch}.json"));
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&input_path).unwrap()).unwrap();
    let baseline: rpmx_root::root::RootLock =
        serde_json::from_value(record["inputs"]["root"].clone()).unwrap();
    assert!(!baseline
        .packages
        .values()
        .flatten()
        .any(|package| package.name == "probe-test-tool"));
    assert_eq!(record["inputs"]["tests"].as_object().unwrap().len(), 1);
    let destination = work.join("binary-worker/cas");
    // The complete inventory has no knowledge of build, sysroot or test roles.
    for checksum in record["payloads"].as_array().unwrap() {
        let bytes = rpmx_store::cas::load(&cache, checksum.as_str().unwrap()).unwrap();
        rpmx_store::cas::store(&destination, &bytes).unwrap();
    }
    rpmx_root::root::remove_tree(cache.parent().unwrap()).unwrap();
    std::fs::remove_dir_all(&repo).unwrap();
    std::fs::remove_file(&profile).unwrap();
    std::fs::remove_file(&recipe).unwrap();
    let srpm = std::fs::read_dir(&prepared)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".src.rpm"))
        .unwrap();
    let rebuilt = work.join("rebuilt");
    let mut request = ApplicationRequest::rebuild(srpm, &rebuilt);
    let Operation::Rebuild(input) = &mut request.operation else {
        unreachable!()
    };
    input.arch = Some(arch.into());
    input.prepared_inputs = Some(input_path);
    input.lint = false;
    input.index = false;
    let replay_input = input.clone();
    let report = Application::new(ApplicationConfig {
        cache: Some(destination.clone()),
        offline: true,
        ..Default::default()
    })
    .execute(request)
    .unwrap();
    assert!(report.success(), "{report:?}");
    assert_eq!(report.build_tests[0].status, CaseStatus::Passed);
    assert!(report.artifacts.rpms[0].is_file());

    // Keep every base package, but omit the test-only RPM on another worker.
    let missing_cache = work.join("missing-worker/cas");
    for package in baseline.packages.values().flatten() {
        let bytes = rpmx_store::cas::load(&destination, &package.checksum).unwrap();
        rpmx_store::cas::store(&missing_cache, &bytes).unwrap();
    }
    let missing = Application::new(ApplicationConfig {
        cache: Some(missing_cache),
        offline: true,
        ..Default::default()
    })
    .execute(ApplicationRequest {
        operation: Operation::Rebuild(replay_input),
        output: work.join("missing-input"),
    })
    .unwrap();
    assert!(!missing.success());
    assert_eq!(missing.build_tests[0].status, CaseStatus::Failed);
    assert!(missing.artifacts.rpms.is_empty());
    let embedded_cache = work.join("embedded-worker/cas");
    for checksum in record["payloads"].as_array().unwrap() {
        let bytes = rpmx_store::cas::load(&destination, checksum.as_str().unwrap()).unwrap();
        rpmx_store::cas::store(&embedded_cache, &bytes).unwrap();
    }
    let mut request =
        ApplicationRequest::rebuild(&local.artifacts.srpms[0], work.join("embedded-rebuilt"));
    let Operation::Rebuild(input) = &mut request.operation else {
        unreachable!()
    };
    input.arch = Some(arch.into());
    input.lint = false;
    input.index = false;
    let embedded = Application::new(ApplicationConfig {
        cache: Some(embedded_cache),
        offline: true,
        ..Default::default()
    })
    .execute(request)
    .unwrap();
    assert!(embedded.success(), "{embedded:?}");
    assert_eq!(embedded.build_tests[0].status, CaseStatus::Passed);
}
