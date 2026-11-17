use std::fs;
use std::num::{NonZeroU32, NonZeroU64};

use rpmx_core::source::{context_root, pack_snapshot, stage_context, unpack_snapshot};

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
fn snapshot_preserves_empty_directories() {
    let work = tempfile::tempdir().expect("work");
    fs::create_dir_all(work.path().join("empty")).expect("empty");
    let out = tempfile::tempdir().expect("out");
    let snapshot = out.path().join("snapshot.tar.gz");
    pack_snapshot(work.path(), &snapshot, 1_700_000_000).expect("pack");

    let restore = tempfile::tempdir().expect("restore");
    unpack_snapshot(&snapshot, restore.path()).expect("unpack");
    assert!(restore.path().join("empty").is_dir());
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

fn apply_patch_fixture(
    source: &[u8],
    patches: &[&str],
    mutate_after_load: bool,
) -> Option<(tempfile::TempDir, rpmx_core::error::Result<()>)> {
    use rpmx_core::source::{assemble_fetched, SourceOptions};

    if std::process::Command::new("bwrap")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("bwrap not installed; skipping patch test");
        return None;
    }
    if std::process::Command::new("patch")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("patch not installed; skipping patch test");
        return None;
    }

    let work = tempfile::tempdir().expect("work");
    let stage = tempfile::tempdir().expect("stage");
    let recipe = tempfile::tempdir().expect("recipe");
    fs::write(work.path().join("file.txt"), source).expect("source");
    let mut yaml = String::from("schema: 2\npackage:\n  name: probe\n  version: '1'\n  release: 1\n  summary: probe\n  license: MIT\npatches:\n");
    for (index, contents) in patches.iter().enumerate() {
        let file = format!("fix-{index}.patch");
        fs::write(recipe.path().join(&file), contents).expect("patch");
        yaml.push_str(&format!("  - file: {file}\n    strip: 1\n"));
    }
    yaml.push_str("outputs:\n  - name: probe\n    metapackage: true\n");
    let recipe_path = recipe.path().join("package.yaml");
    fs::write(&recipe_path, yaml).expect("recipe");
    if mutate_after_load {
        // A contained symlink keeps the declared name distinct from its canonical target.
        fs::rename(
            recipe.path().join("fix-0.patch"),
            recipe.path().join("target.patch"),
        )
        .expect("patch target");
        std::os::unix::fs::symlink("target.patch", recipe.path().join("fix-0.patch"))
            .expect("contained patch symlink");
    }
    let bundle = rpmx_core::load_recipe_bundle(&recipe_path).expect("capture recipe");
    let arch = rpmx_core::arch::Arch::current().expect("host arch");
    let plan = rpmx_core::plan_with_options(
        &bundle.recipe,
        rpmx_core::PlanOptions {
            arch: arch.as_str(),
            step_root: None,
            allow_builtin_override: false,
            dist_tag: None,
            build_release: None,
        },
    )
    .expect("plan");
    if mutate_after_load {
        // This valid replacement would produce different source if assembly reopened it.
        fs::write(
            recipe.path().join("fix-0.patch"),
            "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+changed-after-load\n",
        )
        .expect("mutate live checkout");
    }
    let output = tempfile::tempdir().expect("output");
    let logs = rpmx_core::runner::OutputLogs {
        stdout: fs::File::create(output.path().join("stdout.log")).expect("stdout"),
        stderr: fs::File::create(output.path().join("stderr.log")).expect("stderr"),
    };
    let result = assemble_fetched(
        &SourceOptions {
            recipe: &bundle.recipe,
            recipe_files: &bundle.files,
            logs: &logs,
            plan: &plan,
            cache: output.path(),
            proxy: None,
            offline: true,
            base_dir: recipe.path(),
            workspace: work.path(),
            stage: stage.path(),
            prefix: "/usr",
            libdir: "/usr/lib64",
            output: output.path(),
            source_date_epoch: Some(1_700_000_000),
            source_date_epoch_default: None,
            jobs: NonZeroU32::new(1).expect("jobs"),
            step_timeout: NonZeroU64::new(30).expect("timeout"),
            root: None,
            appliance: None,
            exec: arch,
            sysroot: None,
            cross: None,
            runner: "bwrap",
            recipe_dir: recipe.path(),
        },
        &[],
        1_700_000_000,
    )
    .map(|snapshot| {
        let restored = tempfile::tempdir().expect("restore snapshot");
        unpack_snapshot(&snapshot, restored.path()).expect("snapshot");
        assert_eq!(
            fs::read(restored.path().join("file.txt")).expect("snapshot bytes"),
            fs::read(work.path().join("file.txt")).expect("workspace bytes")
        );
    })
    .map_err(|failure| *failure.primary);
    Some((work, result))
}

#[test]
fn ordered_exact_patches_are_applied_and_removed_from_the_workspace() {
    let Some((work, result)) = apply_patch_fixture(
        b"old\n",
        &[
            "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+middle\n",
            "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-middle\n+new\n",
        ],
        false,
    ) else {
        return;
    };
    result.expect("apply ordered patches");
    assert_eq!(
        fs::read(work.path().join("file.txt")).expect("patched"),
        b"new\n"
    );
    assert!(!work.path().join(".rpmx-patches").exists());
}

#[test]
fn native_patches_reject_fuzzy_context_without_changing_the_source() {
    let source = b"different context\nold\ntrailing context\n";
    let Some((work, result)) = apply_patch_fixture(source, &[
        "--- a/file.txt\n+++ b/file.txt\n@@ -1,3 +1,3 @@\n leading context\n-old\n+new\n trailing context\n",
    ], false) else { return; };
    result.expect_err("mismatched context must fail instead of applying with fuzz");
    assert_eq!(
        fs::read(work.path().join("file.txt")).expect("source"),
        source
    );
}

#[test]
fn native_patches_reject_reversed_or_already_applied_changes() {
    let Some((work, result)) = apply_patch_fixture(
        b"new\n",
        &["--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n"],
        false,
    ) else {
        return;
    };
    result.expect_err("already applied patch must fail instead of being reversed");
    assert_eq!(
        fs::read(work.path().join("file.txt")).expect("source"),
        b"new\n"
    );
}

#[test]
fn source_assembly_uses_captured_patch_bytes_after_the_checkout_changes() {
    let Some((work, result)) = apply_patch_fixture(
        b"old\n",
        &["--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-old\n+new\n"],
        true,
    ) else {
        return;
    };
    result.expect("assemble captured patch into snapshot");
    assert_eq!(
        fs::read(work.path().join("file.txt")).expect("captured result"),
        b"new\n"
    );
}

#[test]
fn context_is_copied_to_the_workspace_root() {
    let recipe = tempfile::tempdir().expect("recipe");
    let context = recipe.path().join("files");
    fs::create_dir_all(context.join("nested")).expect("nested");
    fs::create_dir_all(context.join("empty")).expect("empty");
    fs::write(context.join("os-release"), b"ID=probe\n").expect("os-release");
    fs::write(context.join("nested/config"), b"value\n").expect("config");
    std::os::unix::fs::symlink("os-release", context.join("alias")).expect("symlink");

    let workspace = tempfile::tempdir().expect("workspace");
    stage_context(Some("./files"), recipe.path(), workspace.path()).expect("stage context");

    assert_eq!(
        fs::read(workspace.path().join("os-release")).expect("os-release"),
        b"ID=probe\n"
    );
    assert_eq!(
        fs::read(workspace.path().join("nested/config")).expect("config"),
        b"value\n"
    );
    assert!(workspace.path().join("empty").is_dir());
    let alias = fs::symlink_metadata(workspace.path().join("alias")).expect("alias");
    assert!(alias.file_type().is_symlink());
    assert_eq!(
        fs::read_link(workspace.path().join("alias")).expect("target"),
        std::path::Path::new("os-release")
    );
}

#[test]
fn context_rejects_a_path_that_escapes_the_recipe() {
    let recipe = tempfile::tempdir().expect("recipe");
    let outside = tempfile::tempdir().expect("outside");
    fs::write(outside.path().join("secret"), b"secret").expect("secret");

    let error = context_root(recipe.path(), "../outside").expect_err("escape");
    assert_eq!(error.code, "RPMX-SOURCE-033");

    let error = context_root(recipe.path(), ".").expect_err("recipe root");
    assert_eq!(error.code, "RPMX-SOURCE-033");
}

#[test]
fn context_refuses_to_overwrite_an_upstream_source() {
    let recipe = tempfile::tempdir().expect("recipe");
    let context = recipe.path().join("files");
    fs::create_dir_all(&context).expect("context");
    fs::write(context.join("os-release"), b"context\n").expect("context file");

    let workspace = tempfile::tempdir().expect("workspace");
    fs::write(workspace.path().join("os-release"), b"upstream\n").expect("upstream");

    let error =
        stage_context(Some("files"), recipe.path(), workspace.path()).expect_err("collision");
    assert_eq!(error.code, "RPMX-SOURCE-034");
    assert_eq!(
        fs::read(workspace.path().join("os-release")).expect("upstream"),
        b"upstream\n"
    );
}
