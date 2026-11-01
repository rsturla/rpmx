use std::fs;
use std::num::{NonZeroU32, NonZeroU64};

use rpmx_core::source::{pack_snapshot, unpack_snapshot};

#[test]
fn snapshot_round_trips_and_is_deterministic() {
    let work = tempfile::tempdir().expect("work");
    fs::write(work.path().join("a.txt"), b"a").expect("a");
    fs::create_dir_all(work.path().join("vendor")).expect("dir");
    fs::write(work.path().join("vendor/b.txt"), b"b").expect("b");

    let out = tempfile::tempdir().expect("out");
    let first = out.path().join("first.tar.gz");
    let second = out.path().join("second.tar.gz");
    pack_snapshot(work.path(), &first, 1_700_000_000).expect("pack first");
    pack_snapshot(work.path(), &second, 1_700_000_000).expect("pack second");
    assert_eq!(
        fs::read(&first).expect("first"),
        fs::read(&second).expect("second")
    );

    let restore = tempfile::tempdir().expect("restore");
    unpack_snapshot(&first, restore.path()).expect("unpack");
    assert_eq!(fs::read(restore.path().join("a.txt")).expect("a"), b"a");
    assert_eq!(
        fs::read(restore.path().join("vendor/b.txt")).expect("b"),
        b"b"
    );
}

#[test]
fn snapshot_preserves_symlinks() {
    let work = tempfile::tempdir().expect("work");
    fs::write(work.path().join("a.txt"), b"a").expect("a");
    std::os::unix::fs::symlink("a.txt", work.path().join("alias")).expect("symlink");

    let out = tempfile::tempdir().expect("out");
    let snapshot = out.path().join("snapshot.tar.gz");
    pack_snapshot(work.path(), &snapshot, 1_700_000_000).expect("pack");

    let restore = tempfile::tempdir().expect("restore");
    unpack_snapshot(&snapshot, restore.path()).expect("unpack");
    let metadata = fs::symlink_metadata(restore.path().join("alias")).expect("alias");
    assert!(metadata.file_type().is_symlink());
    assert_eq!(
        fs::read_link(restore.path().join("alias")).expect("target"),
        std::path::Path::new("a.txt")
    );
}

#[test]
fn snapshot_unpack_does_not_write_through_symlinks() {
    let outside = tempfile::tempdir().expect("outside");
    let archive = tempfile::tempdir().expect("archive");
    let snapshot = archive.path().join("snapshot.tar.gz");

    let builder = {
        let file = std::fs::File::create(&snapshot).expect("snapshot");
        let gz = flate2::GzBuilder::new().write(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_path("link").expect("link path");
        header.set_link_name(outside.path()).expect("link target");
        header.set_mode(0o777);
        header.set_size(0);
        header.set_cksum();
        tar.append(&header, std::io::empty()).expect("append link");

        let mut header = tar::Header::new_gnu();
        header.set_path("link/escaped").expect("file path");
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, &b"x"[..]).expect("append file");

        tar.into_inner().expect("tar")
    };
    builder.finish().expect("gzip");

    let restore = tempfile::tempdir().expect("restore");
    let _ = unpack_snapshot(&snapshot, restore.path());
    assert!(
        !outside.path().join("escaped").exists(),
        "unpack wrote through a symlink outside the workspace"
    );
}

#[test]
fn snapshot_with_an_oversized_extension_header_is_rejected() {
    use std::io::Write;

    let archive = tempfile::tempdir().expect("archive");
    let snapshot = archive.path().join("snapshot.tar.gz");
    let file = std::fs::File::create(&snapshot).expect("snapshot");
    let mut gz = flate2::GzBuilder::new().write(file, flate2::Compression::default());
    let mut header = [0u8; 512];
    header[156] = b'L';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let size = format!("{:011o}\0", 2 * 1024 * 1024);
    header[124..136].copy_from_slice(size.as_bytes());
    gz.write_all(&header).expect("header");
    gz.finish().expect("gzip");

    let restore = tempfile::tempdir().expect("restore");
    let error = unpack_snapshot(&snapshot, restore.path()).expect_err("extension header cap");
    assert_eq!(error.code, "RPMX-SOURCE-031");
    assert!(
        error.message.contains("extension header"),
        "{}",
        error.message
    );
    assert!(
        restore.path().read_dir().expect("restore").next().is_none(),
        "a rejected archive must not write into the workspace"
    );
}

#[test]
fn patches_are_applied_and_removed_from_the_workspace() {
    use rpmx_core::model::Patch;
    use rpmx_core::runner::RunnerKind;
    use rpmx_core::sandbox::{Sandbox, SandboxConfig};

    if std::process::Command::new("bwrap")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("bwrap not installed; skipping patch test");
        return;
    }
    if std::process::Command::new("patch")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("patch not installed; skipping patch test");
        return;
    }

    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");
    let recipe = tempfile::tempdir().expect("recipe");
    fs::write(work.path().join("file.txt"), b"old\n").expect("source");
    fs::write(
        recipe.path().join("fix.patch"),
        b"--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n",
    )
    .expect("patch");

    let sandbox = Sandbox::new(SandboxConfig {
        runner: RunnerKind::Bwrap,
        host: rpmx_core::arch::Arch::current().expect("host arch"),
        target: rpmx_core::arch::Arch::current().expect("target arch"),
        workspace: work.path(),
        stage: stage.path(),
        root: None,
        image: None,
        source_date_epoch: 1_700_000_000,
        jobs: NonZeroU32::new(1).expect("jobs"),
        timeout: NonZeroU64::new(3600).expect("timeout"),
        debuginfo: false,
    })
    .expect("sandbox");
    rpmx_core::source::apply_patches(
        &sandbox,
        recipe.path(),
        &[Patch {
            file: "fix.patch".to_string(),
            strip: 1,
            reason: None,
        }],
        work.path(),
    )
    .expect("apply patch");

    assert_eq!(
        fs::read(work.path().join("file.txt")).expect("patched"),
        b"new\n"
    );
    assert!(!work.path().join(".rpmx-patches").exists());
}
