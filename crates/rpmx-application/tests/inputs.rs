use rpmx_application::*;

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
        }),
        output: directory.join("prepared"),
    })
    .unwrap();
    assert!(report.success(), "{report:?}");
    assert_eq!(report.artifacts.srpms.len(), 1);
    assert!(!directory.join("roots").exists());
}
