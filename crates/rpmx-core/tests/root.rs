use std::collections::BTreeMap;

use rpmx_core::pkgmanifest;
use rpmx_core::profile::{parse_profile, Repository, ResolveOptions};
use rpmx_core::root::{
    closure_digest, load_lock, write_lock, LockedPackage, PackageOrigin, RootLock,
};

fn options() -> ResolveOptions {
    ResolveOptions::default()
}

fn package(name: &str, checksum: &str) -> LockedPackage {
    LockedPackage {
        name: name.to_string(),
        origin: PackageOrigin::Repository {
            repository: "fedora".into(),
            location: None,
        },
        evr: "1-1".to_string(),
        checksum: format!(
            "sha512:{}",
            checksum.strip_prefix("sha512:").unwrap().repeat(64)
        ),
        size: 10,
        srpm: None,
        module: None,
    }
}

fn repository() -> Repository {
    Repository {
        id: "fedora".to_string(),
        baseurl: Some("https://example.invalid/fedora".to_string()),
        metalink: None,
        mirrorlist: None,
        gpgkey: None,
        repo_gpgcheck: false,
        exclude: Vec::new(),
        priority: None,
        auth: None,
    }
}

fn lock() -> RootLock {
    let mut packages = BTreeMap::new();
    packages.insert("x86_64".to_string(), vec![package("bash", "sha512:aa")]);
    packages.insert("noarch".to_string(), vec![package("tzdata", "sha512:bb")]);
    let repositories = vec![repository()];
    let archs = vec!["x86_64".to_string()];
    let digest = closure_digest("44", &archs, &repositories, &packages);
    RootLock {
        document: "rpmx-root-lock".to_string(),
        version: rpmx_core::root::VERSION,
        profile: "fedora-44".to_string(),
        archs,
        releasever: "44".to_string(),
        dist_tag: None,
        options: options(),
        prefix: "/usr".to_string(),
        libdir: "/usr/lib64".to_string(),
        sbom: true,
        repositories,
        packages,
        closure_digest: digest,
        root_digest: None,
        rpmdb_digest: None,
        builder: None,
        seed: None,
        generation: None,
        requirement: None,
    }
}

#[test]
fn closure_digest_is_order_independent_and_content_sensitive() {
    let repositories = vec![repository()];
    let archs = vec!["x86_64".to_string()];
    let first = {
        let mut packages = BTreeMap::new();
        packages.insert(
            "x86_64".to_string(),
            vec![package("bash", "sha512:aa"), package("glibc", "sha512:bb")],
        );
        closure_digest("44", &archs, &repositories, &packages)
    };
    let second = {
        let mut packages = BTreeMap::new();
        packages.insert(
            "x86_64".to_string(),
            vec![package("glibc", "sha512:bb"), package("bash", "sha512:aa")],
        );
        closure_digest("44", &archs, &repositories, &packages)
    };
    assert_eq!(first, second);

    let different = {
        let mut packages = BTreeMap::new();
        packages.insert(
            "x86_64".to_string(),
            vec![package("bash", "sha512:cc"), package("glibc", "sha512:bb")],
        );
        closure_digest("44", &archs, &repositories, &packages)
    };
    assert_ne!(first, different);
}

#[test]
fn closure_digest_changes_with_releasever() {
    let packages = lock().packages;
    let repositories = vec![repository()];
    let archs = vec!["x86_64".to_string()];
    assert_ne!(
        closure_digest("44", &archs, &repositories, &packages),
        closure_digest("45", &archs, &repositories, &packages)
    );
}

#[test]
fn lock_round_trips_through_pkgmanifest_without_losing_the_closure() {
    let original = lock();
    let manifest = pkgmanifest::from_lock(&original).unwrap();
    let text = serde_yaml_ng::to_string(&manifest).unwrap();
    let parsed: pkgmanifest::RpmPackageManifest = serde_yaml_ng::from_str(&text).unwrap();
    let imported = pkgmanifest::to_lock(&parsed, "fedora-44", Some("x86_64"), "44").unwrap();
    assert_eq!(original.closure_digest, imported.closure_digest);
    assert_eq!(original.packages, imported.packages);
}

#[test]
fn load_lock_rejects_a_tampered_closure() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("root.lock.yaml");
    let mut tampered = lock();
    write_lock(&path, &tampered).unwrap();
    assert!(load_lock(&path).is_ok());

    tampered.packages.get_mut("x86_64").unwrap()[0].checksum =
        format!("sha512:{}", "cc".repeat(64));
    assert!(
        write_lock(&path, &tampered).is_err(),
        "writers must reject an invalid closure too"
    );
    std::fs::write(&path, serde_yaml_ng::to_string(&tampered).unwrap()).unwrap();
    let error = load_lock(&path).unwrap_err();
    assert_eq!(error.code, "RPMX-ROOT-006");
    assert!(error.message.contains("closure digest mismatch"));
}

const MINIMAL_PROFILE: &str = r#"
document: rpmx-profile
version: 1
name: test
releasever: "44"
dist-tag: .fc44
archs: [x86_64]
repositories:
  - id: fedora
    baseurl: https://example.invalid/fedora
packages:
  install: [bash]
"#;

#[test]
fn profile_accepts_the_minimal_document() {
    let profile = parse_profile(MINIMAL_PROFILE).unwrap();
    assert_eq!(profile.arch(), "x86_64");
    assert_eq!(profile.dist_tag().unwrap().as_deref(), Some(".fc44"));
    assert!(!profile.options.weak_deps);
    assert!(!profile.options.docs);
    assert_eq!(
        profile.builder_image(),
        "registry.fedoraproject.org/fedora:44"
    );
}

#[test]
fn profile_rejects_unknown_fields() {
    let text = MINIMAL_PROFILE.replace("packages:", "extra: true\npackages:");
    let error = parse_profile(&text).unwrap_err();
    assert_eq!(error.code, "RPMX-ROOT-006");
}

#[test]
fn profile_rejects_two_repository_sources() {
    let text = MINIMAL_PROFILE.replace(
        "    baseurl: https://example.invalid/fedora",
        "    baseurl: https://example.invalid/fedora\n    metalink: https://example.invalid/metalink",
    );
    let error = parse_profile(&text).unwrap_err();
    assert!(error.message.contains("exactly one of"));
}

#[test]
fn profile_accepts_multiple_archs_and_derives_a_target() {
    let text = MINIMAL_PROFILE.replace("archs: [x86_64]", "archs: [x86_64, aarch64]");
    let profile = parse_profile(&text).expect("multi-arch profile");
    assert_eq!(
        profile.archs,
        vec!["x86_64".to_string(), "aarch64".to_string()]
    );
    let target = profile.for_target("aarch64").expect("derived target");
    assert_eq!(target.archs, vec!["aarch64".to_string()]);
}

#[test]
fn profile_rejects_an_invalid_dist_tag() {
    let text = MINIMAL_PROFILE.replace("dist-tag: .fc44", "dist-tag: fc/44");
    let error = parse_profile(&text).unwrap_err();
    assert_eq!(error.code, "RPMX-ROOT-006");
    assert!(error.message.contains("dist tag"));
}

#[test]
fn profile_rejects_a_foreign_document() {
    let text = MINIMAL_PROFILE.replace("document: rpmx-profile", "document: rpmx-recipe");
    let error = parse_profile(&text).unwrap_err();
    assert!(error.message.contains("rpmx-profile"));
}

#[test]
fn profile_accepts_authenticated_repository_credentials() {
    let text = MINIMAL_PROFILE.replace(
        "    baseurl: https://example.invalid/fedora",
        "    baseurl: https://example.invalid/fedora\n\
         \x20   repo-gpgcheck: true\n\
         \x20   auth:\n\
         \x20     ca: /etc/pki/entitlement/ca.pem\n\
         \x20     cert: env:RHEL_CLIENT_CERT\n\
         \x20     key: env:RHEL_CLIENT_KEY",
    );
    let profile = parse_profile(&text).unwrap();
    let repository = &profile.repositories[0];
    assert!(repository.repo_gpgcheck);
    let auth = repository.auth.as_ref().expect("auth");
    assert_eq!(auth.cert.as_deref(), Some("env:RHEL_CLIENT_CERT"));
}

#[test]
fn profile_rejects_cert_without_key() {
    let text = MINIMAL_PROFILE.replace(
        "    baseurl: https://example.invalid/fedora",
        "    baseurl: https://example.invalid/fedora\n\
         \x20   auth:\n\
         \x20     cert: env:RHEL_CLIENT_CERT",
    );
    let error = parse_profile(&text).unwrap_err();
    assert!(error.message.contains("auth.cert and auth.key"));
}

#[test]
fn profile_rejects_a_literal_password() {
    let text = MINIMAL_PROFILE.replace(
        "    baseurl: https://example.invalid/fedora",
        "    baseurl: https://example.invalid/fedora\n\
         \x20   auth:\n\
         \x20     username: alice\n\
         \x20     password: s3cret",
    );
    let error = parse_profile(&text).unwrap_err();
    assert!(error.message.contains("env:NAME reference"));
}

#[test]
fn profile_rejects_credentials_in_a_repository_url() {
    let text = MINIMAL_PROFILE.replace(
        "https://example.invalid/fedora",
        "https://alice:s3cret@example.invalid/fedora",
    );
    let error = parse_profile(&text).unwrap_err();
    assert_eq!(error.code, "RPMX-ROOT-006");
    assert!(error.message.contains("use the auth block"));
}

#[test]
fn profile_rejects_credentials_in_a_proxy_url() {
    let text = MINIMAL_PROFILE.replace(
        "    baseurl: https://example.invalid/fedora",
        "    baseurl: https://example.invalid/fedora\n\
         \x20   auth:\n\
         \x20     proxy: http://alice:s3cret@proxy.invalid:3128",
    );
    let error = parse_profile(&text).unwrap_err();
    assert!(error.message.contains("auth.proxy"));
}

#[test]
fn profile_rejects_scheme_less_proxy_credentials() {
    let text = MINIMAL_PROFILE.replace(
        "    baseurl: https://example.invalid/fedora",
        "    baseurl: https://example.invalid/fedora\n\
         \x20   auth:\n\
         \x20     proxy: alice:s3cret@proxy.invalid:3128",
    );
    let error = parse_profile(&text).unwrap_err();
    assert!(error.message.contains("auth.proxy"));
}

#[test]
fn profile_rejects_a_multiline_repository_url() {
    let text = MINIMAL_PROFILE.replace(
        "https://example.invalid/fedora",
        "\"https://example.invalid/fedora\\nsslverify=0\"",
    );
    let error = parse_profile(&text).unwrap_err();
    assert_eq!(error.code, "RPMX-ROOT-006");
    assert!(error.message.contains("control character"));
}

#[test]
fn profile_rejects_file_and_env_combined_in_a_password_reference() {
    let text = MINIMAL_PROFILE.replace(
        "    baseurl: https://example.invalid/fedora",
        "    baseurl: https://example.invalid/fedora\n\
         \x20   auth:\n\
         \x20     username: alice\n\
         \x20     password: file://env:FOO",
    );
    let error = parse_profile(&text).unwrap_err();
    assert!(error.message.contains("env:NAME reference"));
}
