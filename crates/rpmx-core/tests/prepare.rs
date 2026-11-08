use std::fs;

use rpmx_core::binary_package::{prepare_binary_packages, BinaryPackageOptions};
use rpmx_core::source_package::{prepare_source_packages, SourcePackageOptions};
mod common;

use rpmx_core::guest::CrossToolchain;
use rpmx_core::{parse_recipe, plan};

fn root_lock(arch: &str) -> rpmx_core::root::RootLock {
    rpmx_core::root::RootLock {
        document: rpmx_core::root::DOCUMENT.to_string(),
        version: rpmx_core::root::VERSION,
        profile: "test".to_string(),
        archs: vec![arch.to_string()],
        releasever: "44".to_string(),
        dist_tag: None,
        options: Default::default(),
        prefix: "/usr".to_string(),
        libdir: "/usr/lib64".to_string(),
        sbom: true,
        repositories: Vec::new(),
        packages: std::collections::BTreeMap::new(),
        closure_digest: "sha512:00".to_string(),
        root_digest: Some("sha512:00".to_string()),
        builder: None,
        seed: None,
        generation: None,
        requirement: None,
    }
}

const RECIPE: &str = r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
outputs:
  - name: demo
    files:
      include: ["/usr/bin/**"]
  - name: demo-doc
    files:
      include: ["/usr/share/doc/**"]
"#;

#[test]
fn source_package_contains_recipe_lock_manifest_and_sources() {
    let (recipe, plan) = common::load(RECIPE);
    let sources = tempfile::tempdir().expect("tempdir");
    fs::write(sources.path().join("demo-1.0.tar.gz"), b"source").expect("source");

    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &recipe,
        plan: &plan,
        recipe_bytes: RECIPE.as_bytes(),
        recipe_digest: None,
        recipe_includes: &[],
        step_root: None,
        sources: sources.path(),
        source_date_epoch: 1_700_000_000,
        root_lock: None,
        sysroot_lock: None,
        cross: None,
        flags: None,
        sbom: true,
        replaced: None,
    })
    .expect("prepare SRPM");

    assert_eq!(packages.len(), 1);
    let files: Vec<_> = packages[0]
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert!(files.contains(&"package.yaml"));
    assert!(files.contains(&"package.lock.json"));
    assert!(files.contains(&"RPMX-MANIFEST.json"));
    assert!(files.contains(&"RPMX-BUNDLED.json"));
    assert!(files.contains(&"demo-1.0.tar.gz"));
}

#[test]
fn source_package_rejects_reserved_source_names() {
    let (recipe, plan) = common::load(RECIPE);
    let sources = tempfile::tempdir().expect("tempdir");
    fs::write(sources.path().join("package.yaml"), b"collision").expect("source");

    let error = prepare_source_packages(SourcePackageOptions {
        recipe: &recipe,
        plan: &plan,
        recipe_bytes: RECIPE.as_bytes(),
        recipe_digest: None,
        recipe_includes: &[],
        step_root: None,
        sources: sources.path(),
        source_date_epoch: 1_700_000_000,
        root_lock: None,
        sysroot_lock: None,
        cross: None,
        flags: None,
        sbom: true,
        replaced: None,
    })
    .expect_err("reserved name is rejected");
    assert_eq!(error.code, "RPMX-SRPM-007");
}

#[test]
fn binary_package_prepares_one_package_per_output() {
    let (recipe, plan) = common::load(RECIPE);
    let stage = common::stage();

    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: stage.path(),
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
        bundled: &rpmx_components::Inventory::empty(),
    })
    .expect("prepare RPMs");

    assert_eq!(packages.len(), 2);
    assert_eq!(packages[0].name, "demo");
    assert_eq!(packages[0].files[0].path, "/usr/bin/demo");
    assert_eq!(packages[1].name, "demo-doc");
    assert_eq!(packages[1].files[0].path, "/usr/share/doc/demo/README");
    assert_eq!(packages[0].source_rpm, "demo-1.0-1.src.rpm");
}

#[test]
fn lock_records_builtin_step_digests() {
    let yaml = r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
build:
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
    let sources = tempfile::tempdir().expect("tempdir");
    fs::write(sources.path().join("demo-1.0.tar.gz"), b"source").expect("source");

    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &recipe,
        plan: &plan,
        recipe_bytes: yaml.as_bytes(),
        recipe_digest: None,
        recipe_includes: &[],
        step_root: None,
        sources: sources.path(),
        source_date_epoch: 1_700_000_000,
        root_lock: None,
        sysroot_lock: None,
        cross: None,
        flags: None,
        sbom: true,
        replaced: None,
    })
    .expect("prepare SRPM");

    let lock = packages[0]
        .files
        .iter()
        .find(|file| file.path == "package.lock.json")
        .expect("lock");
    let text = String::from_utf8(lock.data.clone()).expect("utf8");
    assert!(text.contains("\"name\": \"go/build\""));

    let steps = packages[0]
        .files
        .iter()
        .find(|file| file.path == "RPMX-STEPS.json")
        .expect("embedded steps");
    let steps = String::from_utf8(steps.data.clone()).expect("utf8");
    assert!(steps.contains("\"go/build\""));
}

#[test]
fn binary_dependencies_are_generated_from_elf_and_pkgconfig() {
    const RECIPE: &str = r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
outputs:
  - name: demo
    files:
      include: ["/**"]
"#;
    let (recipe, plan) = common::load(RECIPE);
    let stage = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(stage.path().join("usr/bin")).expect("bin");
    fs::create_dir_all(stage.path().join("usr/lib64/pkgconfig")).expect("pkgconfig");
    fs::copy(
        std::env::current_exe().expect("current exe"),
        stage.path().join("usr/bin/demo"),
    )
    .expect("copy exe");
    fs::write(
        stage.path().join("usr/lib64/pkgconfig/demo.pc"),
        b"Name: demo\nVersion: 1.2.3\n",
    )
    .expect("pc");

    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: stage.path(),
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
        bundled: &rpmx_components::Inventory::empty(),
    })
    .expect("prepare");

    assert!(
        packages[0]
            .dependencies
            .requires
            .iter()
            .any(|dependency| dependency.starts_with("libc.so")),
        "requires: {:?}",
        packages[0].dependencies.requires
    );
    assert!(
        packages[0]
            .dependencies
            .provides
            .iter()
            .any(|dependency| dependency == "pkgconfig(demo) = 1.2.3"),
        "provides: {:?}",
        packages[0].dependencies.provides
    );
}

#[test]
fn output_file_flags_mark_license_doc_and_config() {
    const RECIPE: &str = r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
outputs:
  - name: demo
    files:
      include: ["/**"]
      license: ["/usr/share/licenses/**"]
      doc: ["/usr/share/doc/**"]
      config: ["/etc/**"]
      noreplace: ["/etc/**"]
"#;
    let (recipe, plan) = common::load(RECIPE);
    let stage = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(stage.path().join("usr/share/licenses/demo")).expect("licenses");
    fs::create_dir_all(stage.path().join("usr/share/doc/demo")).expect("docs");
    fs::create_dir_all(stage.path().join("etc")).expect("etc");
    fs::write(stage.path().join("usr/share/licenses/demo/LICENSE"), b"MIT").expect("license");
    fs::write(stage.path().join("usr/share/doc/demo/README"), b"readme").expect("doc");
    fs::write(stage.path().join("etc/demo.conf"), b"setting=1").expect("config");

    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: stage.path(),
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
        bundled: &rpmx_components::Inventory::empty(),
    })
    .expect("prepare");

    let flags = |path: &str| {
        let file = packages[0]
            .files
            .iter()
            .find(|file| file.path == path)
            .expect("file");
        (file.license, file.doc, file.config)
    };
    assert_eq!(
        flags("/usr/share/licenses/demo/LICENSE"),
        (true, false, false)
    );
    assert_eq!(flags("/usr/share/doc/demo/README"), (false, true, false));
    assert_eq!(flags("/etc/demo.conf"), (false, false, true));
}

#[test]
fn source_package_embeds_included_recipe_fragments() {
    use rpmx_core::source_package::RecipeInclude;

    let (recipe, plan) = common::load(RECIPE);
    let sources = tempfile::tempdir().expect("tempdir");
    fs::write(sources.path().join("demo-1.0.tar.gz"), b"source").expect("source");
    let includes = vec![RecipeInclude {
        name: "build.yaml".to_string(),
        bytes: b"build:\n  pipeline: []\n".to_vec(),
    }];

    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &recipe,
        plan: &plan,
        recipe_bytes: RECIPE.as_bytes(),
        recipe_digest: Some("sha512:bundle"),
        recipe_includes: &includes,
        step_root: None,
        sources: sources.path(),
        source_date_epoch: 1_700_000_000,
        root_lock: None,
        sysroot_lock: None,
        cross: None,
        flags: None,
        sbom: true,
        replaced: None,
    })
    .expect("prepare SRPM");

    assert!(packages[0]
        .files
        .iter()
        .any(|file| file.path == "build.yaml"));
    let manifest = packages[0]
        .files
        .iter()
        .find(|file| file.path == "RPMX-MANIFEST.json")
        .expect("manifest");
    let text = String::from_utf8_lossy(&manifest.data);
    assert!(text.contains("sha512:bundle"), "{text}");
}

#[test]
fn every_file_class_contributes_its_dependencies() {
    const RECIPE: &str = r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
outputs:
  - name: demo
    files:
      include: ["/usr/**"]
      exclude: ["/usr/share/doc/**"]
  - name: demo-doc
    files:
      include: ["/usr/share/doc/**"]
"#;
    let (recipe, plan) = common::load(RECIPE);
    let stage = tempfile::tempdir().expect("tempdir");
    let root = stage.path();
    fs::create_dir_all(root.join("usr/bin")).expect("bin");
    fs::create_dir_all(root.join("usr/lib64/python3.12/site-packages/foo")).expect("python");
    fs::create_dir_all(root.join("usr/lib64/python3.12/site-packages/foo-1.0.dist-info"))
        .expect("dist-info");
    fs::create_dir_all(root.join("usr/lib64/pkgconfig")).expect("pkgconfig");
    fs::create_dir_all(root.join("usr/share/applications")).expect("applications");
    fs::create_dir_all(root.join("usr/share/metainfo")).expect("metainfo");
    fs::create_dir_all(root.join("usr/share/perl5")).expect("perl");
    fs::create_dir_all(root.join("usr/lib64/perl5/vendor_perl/auto/Demo")).expect("perl arch");
    fs::create_dir_all(root.join("usr/share/doc/demo")).expect("doc");

    fs::write(root.join("usr/bin/tool"), b"#!/usr/bin/python3\nprint(1)\n").expect("tool");
    fs::write(root.join("usr/bin/python3.12"), b"").expect("interpreter");
    fs::write(
        root.join("usr/lib64/python3.12/site-packages/foo/__init__.py"),
        b"",
    )
    .expect("module");
    fs::write(
        root.join("usr/lib64/python3.12/site-packages/foo-1.0.dist-info/METADATA"),
        b"Metadata-Version: 2.1\nName: foo\nVersion: 1.0\n\
          Requires-Dist: bar>=2.0\n\
          Requires-Dist: testpkg; extra == \"test\"\n",
    )
    .expect("metadata");
    fs::write(
        root.join("usr/lib64/pkgconfig/libdemo.pc"),
        b"Name: libdemo\nVersion: 1.2\nRequires: zlib >= 1.2\n",
    )
    .expect("pc");
    fs::write(
        root.join("usr/share/applications/demo.desktop"),
        b"[Desktop Entry]\nType=Application\nExec=demo\nMimeType=text/plain;\n",
    )
    .expect("desktop");
    fs::write(
        root.join("usr/share/metainfo/demo.metainfo.xml"),
        b"<component/>",
    )
    .expect("metainfo");
    fs::write(
        root.join("usr/share/perl5/Demo.pm"),
        b"package Demo;\nour $VERSION = '0.5';\nuse JSON::PP;\n",
    )
    .expect("perl");
    fs::write(
        root.join("usr/lib64/perl5/vendor_perl/auto/Demo/Demo.so"),
        b"",
    )
    .expect("perl xs");
    fs::write(root.join("usr/share/doc/demo/README"), b"doc").expect("doc");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in ["usr/bin/tool", "usr/bin/python3.12"] {
            let path = root.join(path);
            let mut permissions = fs::metadata(&path).expect("stat").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).expect("chmod");
        }
    }

    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: root,
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
        bundled: &rpmx_components::Inventory::empty(),
    })
    .expect("prepare");
    let demo = packages
        .iter()
        .find(|package| package.name == "demo")
        .expect("demo package");
    let provides = &demo.dependencies.provides;
    let requires = &demo.dependencies.requires;

    for expected in [
        "python(abi) = 3.12",
        "python3.12dist(foo) = 1",
        "pkgconfig(libdemo) = 1.2",
        "application()",
        "application(demo.desktop)",
        "metainfo()",
        "metainfo(demo.metainfo.xml)",
        "perl(Demo) = 0.5",
    ] {
        assert!(
            provides.iter().any(|provide| provide == expected),
            "missing provide {expected:?} in {provides:?}"
        );
    }
    for expected in [
        "/usr/bin/python3",
        "pkgconfig(zlib) >= 1.2",
        "python3.12dist(bar) >= 2",
        "perl(JSON::PP)",
    ] {
        assert!(
            requires.iter().any(|require| require == expected),
            "missing require {expected:?} in {requires:?}"
        );
    }
    assert!(
        !requires.iter().any(|require| require.contains("testpkg")),
        "extras must not leak into the base package: {requires:?}"
    );
    assert!(
        !requires
            .iter()
            .any(|require| require == "python(abi) = 3.12"),
        "a package must not require the ABI it ships: {requires:?}"
    );
    assert!(
        !requires
            .iter()
            .any(|require| require == "perl-libs" || require.starts_with("perl(:MODULE_COMPAT_")),
        "generators must not invent a Perl runtime package: {requires:?}"
    );

    let docs = packages
        .iter()
        .find(|package| package.name == "demo-doc")
        .expect("doc package");
    assert!(
        docs.dependencies.provides.is_empty() && docs.dependencies.requires.is_empty(),
        "docs picked up generated dependencies: {:?}",
        docs.dependencies
    );
}

#[test]
fn a_recipe_can_disable_a_generator_and_exclude_a_dependency() {
    const RECIPE: &str = r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
outputs:
  - name: demo
    files:
      include: ["/**"]
    generators:
      pkgconfig: false
      exclude: ["/usr/bin/python3"]
"#;
    let (recipe, plan) = common::load(RECIPE);
    let stage = tempfile::tempdir().expect("tempdir");
    let root = stage.path();
    fs::create_dir_all(root.join("usr/bin")).expect("bin");
    fs::create_dir_all(root.join("usr/lib64/pkgconfig")).expect("pkgconfig");
    fs::write(root.join("usr/bin/tool"), b"#!/usr/bin/python3\n").expect("tool");
    fs::write(
        root.join("usr/lib64/pkgconfig/demo.pc"),
        b"Name: demo\nVersion: 1.0\n",
    )
    .expect("pc");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("usr/bin/tool");
        let mut permissions = fs::metadata(&path).expect("stat").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod");
    }

    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: root,
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
        bundled: &rpmx_components::Inventory::empty(),
    })
    .expect("prepare");
    let dependencies = &packages[0].dependencies;

    assert!(
        !dependencies
            .provides
            .iter()
            .any(|provide| provide.starts_with("pkgconfig(")),
        "provides: {:?}",
        dependencies.provides
    );
    assert!(
        !dependencies
            .requires
            .iter()
            .any(|require| require.starts_with("pkgconfig(")),
        "requires: {:?}",
        dependencies.requires
    );
    assert!(
        !dependencies
            .requires
            .iter()
            .any(|require| require == "/usr/bin/python3"),
        "excluded require is still present: {:?}",
        dependencies.requires
    );
}

#[test]
fn a_cross_source_package_embeds_the_sysroot_and_toolchain() {
    let (recipe, plan) = common::load(RECIPE);
    let sources = tempfile::tempdir().expect("tempdir");
    fs::write(sources.path().join("demo-1.0.tar.gz"), b"source").expect("source");
    let build_root = root_lock("x86_64");
    let sysroot = root_lock("aarch64");
    let mut toolchain = CrossToolchain::new("aarch64-linux-gnu");
    toolchain.cflags = vec!["-mcpu=x".to_string()];
    toolchain.ldflags = vec!["-Wl,-z,now".to_string()];

    let packages = prepare_source_packages(SourcePackageOptions {
        recipe: &recipe,
        plan: &plan,
        recipe_bytes: RECIPE.as_bytes(),
        recipe_digest: None,
        recipe_includes: &[],
        step_root: None,
        sources: sources.path(),
        source_date_epoch: 1_700_000_000,
        root_lock: Some(&build_root),
        sysroot_lock: Some(&sysroot),
        cross: Some(&toolchain),
        flags: None,
        sbom: true,
        replaced: None,
    })
    .expect("prepare cross SRPM");

    let file = |path: &str| {
        packages[0]
            .files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} is missing from the SRPM"))
    };
    let sysroot_text = file("sysroot.lock.json");
    let lock: serde_json::Value =
        serde_json::from_slice(&file("package.lock.json").data).expect("package lock json");
    let cross = lock.get("cross").expect("cross block");
    assert_eq!(cross["triple"], "aarch64-linux-gnu");
    assert_eq!(cross["sysroot"]["path"], "sysroot.lock.json");
    assert_eq!(cross["cflags"][0], "-mcpu=x");
    assert_eq!(cross["ldflags"][0], "-Wl,-z,now");
    // The recorded digest covers the embedded file byte for byte.
    assert_eq!(
        cross["sysroot"]["digest"],
        rpmx_core::srpm::sha512_hex(&sysroot_text.data)
    );
    // A native SRPM has no cross block.
    let native = prepare_source_packages(SourcePackageOptions {
        recipe: &recipe,
        plan: &plan,
        recipe_bytes: RECIPE.as_bytes(),
        recipe_digest: None,
        recipe_includes: &[],
        step_root: None,
        sources: sources.path(),
        source_date_epoch: 1_700_000_000,
        root_lock: Some(&build_root),
        sysroot_lock: None,
        cross: None,
        flags: None,
        sbom: true,
        replaced: None,
    })
    .expect("prepare native SRPM");
    let lock: serde_json::Value = serde_json::from_slice(
        &native[0]
            .files
            .iter()
            .find(|f| f.path == "package.lock.json")
            .unwrap()
            .data,
    )
    .expect("json");
    assert!(lock.get("cross").is_none());
    assert!(native[0]
        .files
        .iter()
        .all(|file| file.path != "sysroot.lock.json"));
}

#[test]
fn explicit_and_discovered_components_become_bundled_provides() {
    let (recipe, plan) = common::load(RECIPE);
    let stage = common::stage();
    let source = tempfile::tempdir().expect("source");
    fs::create_dir_all(source.path().join(".rpmx/bundled/demo")).expect("bundled dir");
    fs::write(
        source.path().join(".rpmx/bundled/demo/zlib.list"),
        "zlib=1.3.1\n",
    )
    .expect("manual list");
    fs::create_dir_all(source.path().join("vendor")).expect("vendor");
    fs::write(
        source.path().join("vendor/modules.txt"),
        "# github.com/spf13/cobra v1.8.1\n",
    )
    .expect("modules");

    let inventory =
        rpmx_core::components::discover(source.path(), &[&plan.cells[0]]).expect("inventory");
    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: stage.path(),
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
        bundled: &inventory,
    })
    .expect("prepare RPMs");

    let provides = &packages[0].dependencies.provides;
    assert!(
        provides.contains(&"bundled(zlib) = 1.3.1".to_string()),
        "{provides:?}"
    );
    assert!(
        provides.contains(&"bundled(golang(github.com/spf13/cobra)) = 1.8.1".to_string()),
        "{provides:?}"
    );
    // The doc output does not carry the component.
    assert!(!packages[1]
        .dependencies
        .provides
        .iter()
        .any(|provide| provide.starts_with("bundled(")));
}

#[test]
fn the_bundled_generator_can_be_disabled_per_output() {
    let recipe = r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
outputs:
  - name: demo
    files:
      include: ["/usr/bin/**", "/usr/share/doc/**"]
    generators:
      bundled: false
"#;
    let (recipe, plan) = common::load(recipe);
    let stage = common::stage();
    let source = tempfile::tempdir().expect("source");
    fs::create_dir_all(source.path().join("vendor")).expect("vendor");
    fs::write(
        source.path().join("vendor/modules.txt"),
        "# github.com/spf13/cobra v1.8.1\n",
    )
    .expect("modules");

    let inventory =
        rpmx_core::components::discover(source.path(), &[&plan.cells[0]]).expect("inventory");
    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: stage.path(),
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
        bundled: &inventory,
    })
    .expect("prepare RPMs");

    assert!(!packages[0]
        .dependencies
        .provides
        .iter()
        .any(|provide| provide.starts_with("bundled(")));
}

#[test]
fn generators_exclude_drops_a_bundled_provide() {
    let recipe = r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
outputs:
  - name: demo
    files:
      include: ["/usr/bin/**", "/usr/share/doc/**"]
    generators:
      exclude:
        - "bundled(crate(serde)) = 1.0.203"
"#;
    let (recipe, plan) = common::load(recipe);
    let stage = common::stage();
    let source = tempfile::tempdir().expect("source");
    fs::create_dir_all(source.path().join("vendor")).expect("vendor");
    fs::write(
        source.path().join("Cargo.lock"),
        r#"version = 4

[[package]]
name = "serde"
version = "1.0.203"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    )
    .expect("cargo lock");

    let inventory =
        rpmx_core::components::discover(source.path(), &[&plan.cells[0]]).expect("inventory");
    let packages = prepare_binary_packages(BinaryPackageOptions {
        recipe: &recipe,
        cell: &plan.cells[0],
        stage: stage.path(),
        arch: "x86_64",
        source_date_epoch: 1_700_000_000,
        bundled: &inventory,
    })
    .expect("prepare RPMs");

    assert!(!packages[0]
        .dependencies
        .provides
        .iter()
        .any(|provide| provide == "bundled(crate(serde)) = 1.0.203"));
}

#[test]
fn a_padded_bundled_name_is_rejected_at_plan_time() {
    let error = common::try_load(
        r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
outputs:
  - name: demo
    files:
      include: ["/usr/bin/**"]
    bundled:
      - name: "zlib "
        version: "1.3.1"
"#,
    )
    .expect_err("padded name");
    assert_eq!(error.code, "RPMX-OUTPUT-004");
}
