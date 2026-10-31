use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, SystemTime};

use rpmx_core::tree::tree_digest;

fn write(path: &std::path::Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn digest_is_stable_across_mtimes() {
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("etc/config");
    write(&path, "hello");

    let first = tree_digest(work.path()).unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_times(
        fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(12345)),
    )
    .unwrap();
    let second = tree_digest(work.path()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn digest_changes_with_content_and_mode() {
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("usr/bin/tool");
    write(&path, "one");
    let first = tree_digest(work.path()).unwrap();

    write(&path, "two");
    let content_changed = tree_digest(work.path()).unwrap();
    assert_ne!(first, content_changed);

    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let mode_changed = tree_digest(work.path()).unwrap();
    assert_ne!(content_changed, mode_changed);
}

#[test]
fn digest_covers_symlinks() {
    let work = tempfile::tempdir().unwrap();
    write(&work.path().join("usr/bin/tool"), "one");
    std::os::unix::fs::symlink("tool", work.path().join("usr/bin/alias")).unwrap();
    let first = tree_digest(work.path()).unwrap();

    fs::remove_file(work.path().join("usr/bin/alias")).unwrap();
    std::os::unix::fs::symlink("other", work.path().join("usr/bin/alias")).unwrap();
    assert_ne!(first, tree_digest(work.path()).unwrap());
}

#[test]
fn digest_ignores_package_manager_state() {
    let work = tempfile::tempdir().unwrap();
    write(&work.path().join("etc/os-release"), "fedora");
    // Create the excluded trees once so their parent directories are part of
    // the first digest; only their volatile contents change afterwards.
    write(&work.path().join("var/log/first"), "a log line");
    write(&work.path().join("var/cache/first"), "cache");
    write(&work.path().join("usr/lib/sysimage/rpm/first"), "db");
    write(&work.path().join("etc/machine-id"), "id");
    let first = tree_digest(work.path()).unwrap();

    write(&work.path().join("var/log/dnf5.log"), "another log line");
    write(&work.path().join("var/cache/ldconfig/aux-cache"), "cache");
    write(&work.path().join("usr/lib/sysimage/rpm/rpmdb.sqlite"), "db");
    std::fs::write(work.path().join("etc/machine-id"), "other").unwrap();
    assert_eq!(first, tree_digest(work.path()).unwrap());
}
