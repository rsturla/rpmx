use rpmx_core::cas;

#[test]
fn store_and_load_round_trip() {
    let work = tempfile::tempdir().unwrap();
    let digest = cas::store(work.path(), b"payload").unwrap();
    assert!(digest.starts_with("sha512:"));
    assert_eq!(cas::load(work.path(), &digest).unwrap(), b"payload");
}

#[test]
fn load_rejects_corrupt_content() {
    let work = tempfile::tempdir().unwrap();
    let digest = cas::store(work.path(), b"payload").unwrap();
    let path = cas::path(work.path(), &digest).unwrap();
    std::fs::write(&path, b"tampered").unwrap();
    let error = cas::load(work.path(), &digest).unwrap_err();
    assert_eq!(error.code, "RPMX-ROOT-011");
    assert!(error.message.contains("corrupt"));
}

#[test]
fn non_sha512_digests_are_rejected() {
    let work = tempfile::tempdir().unwrap();
    let error = cas::path(work.path(), "sha256:abcd").unwrap_err();
    assert_eq!(error.code, "RPMX-ROOT-011");
}

#[test]
fn gc_keeps_referenced_blobs_and_removes_the_rest() {
    let work = tempfile::tempdir().unwrap();
    let keep = cas::store(work.path(), b"keep").unwrap();
    let remove = cas::store(work.path(), b"remove").unwrap();
    let mut referenced = std::collections::BTreeSet::new();
    referenced.insert(keep.clone());

    let report = cas::gc(
        work.path(),
        cas::GcOptions {
            keep: &referenced,
            older_than: std::time::Duration::ZERO,
            dry_run: false,
            now: std::time::SystemTime::now(),
        },
    )
    .unwrap();

    assert_eq!(report.removed, 1);
    assert_eq!(report.blobs, vec![remove.clone()]);
    assert!(cas::load(work.path(), &keep).is_ok());
    assert!(cas::load(work.path(), &remove).is_err());
}

#[test]
fn gc_retains_recent_blobs() {
    let work = tempfile::tempdir().unwrap();
    cas::store(work.path(), b"recent").unwrap();
    let report = cas::gc(
        work.path(),
        cas::GcOptions {
            keep: &std::collections::BTreeSet::new(),
            older_than: std::time::Duration::from_secs(3600),
            dry_run: false,
            now: std::time::SystemTime::now(),
        },
    )
    .unwrap();
    assert_eq!(report.kept, 1);
    assert_eq!(report.removed, 0);
}

#[test]
fn gc_dry_run_does_not_delete() {
    let work = tempfile::tempdir().unwrap();
    let digest = cas::store(work.path(), b"payload").unwrap();
    let report = cas::gc(
        work.path(),
        cas::GcOptions {
            keep: &std::collections::BTreeSet::new(),
            older_than: std::time::Duration::ZERO,
            dry_run: true,
            now: std::time::SystemTime::now(),
        },
    )
    .unwrap();
    assert_eq!(report.removed, 1);
    assert!(cas::load(work.path(), &digest).is_ok());
}

#[test]
fn a_missing_payload_is_reported() {
    let work = tempfile::tempdir().unwrap();
    let digest = format!("sha512:{}", "a".repeat(128));
    let error = cas::load(work.path(), &digest).unwrap_err();
    assert!(error.message.contains("not in the cache"));
}
