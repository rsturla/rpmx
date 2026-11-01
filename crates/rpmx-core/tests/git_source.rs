use std::fs;
use std::path::Path;
use std::process::Command;

use rpmx_core::fetch::{fetch_sources, FetchOptions};
use rpmx_core::load_recipe;

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn repository(parent: &Path) -> String {
    let repo = parent.join("repo");
    fs::create_dir_all(&repo).expect("repo");
    fs::write(repo.join("hello.txt"), b"hello\n").expect("file");
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "hello.txt"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=rpmx",
            "-c",
            "user.email=rpmx@example.invalid",
            "commit",
            "-qm",
            "initial",
        ],
    );
    git(&repo, &["rev-parse", "HEAD"])
}

fn recipe(commit: &str, digest: &str) -> String {
    format!(
        "schema: 1\npackage:\n  name: demo\n  version: \"1.0\"\n  release: 1\n  summary: demo\n  license: MIT\nsources:\n  - name: demo\n    kind: git\n    digest: \"{digest}\"\n    locators:\n      - kind: git\n        url: file://REPO\n        commit: {commit}\n"
    )
}

#[test]
fn git_sources_are_pinned_and_archived_deterministically() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed; skipping git source test");
        return;
    }
    let work = tempfile::tempdir().expect("work");
    let commit = repository(work.path());
    let cache = work.path().join("cache");
    let recipe_path = work.path().join("package.yaml");
    let placeholder = format!("sha512:{}", "0".repeat(128));
    fs::write(
        &recipe_path,
        recipe(&commit, &placeholder).replace(
            "file://REPO",
            &format!("file://{}", work.path().join("repo").display()),
        ),
    )
    .expect("recipe");

    let recipe_value = load_recipe(&recipe_path).expect("load");
    let error = fetch_sources(
        &recipe_value,
        FetchOptions {
            cache: &cache,
            proxy: None,
            offline: false,
            base_dir: work.path(),
        },
    )
    .expect_err("the placeholder digest must mismatch");
    let actual = error
        .message
        .split("got ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("actual digest")
        .to_string();
    assert!(actual.starts_with("sha512:"), "{}", error.message);

    fs::write(
        &recipe_path,
        recipe(&commit, &actual).replace(
            "file://REPO",
            &format!("file://{}", work.path().join("repo").display()),
        ),
    )
    .expect("recipe");
    let recipe_value = load_recipe(&recipe_path).expect("load");
    let fetched = fetch_sources(
        &recipe_value,
        FetchOptions {
            cache: &cache,
            proxy: None,
            offline: false,
            base_dir: work.path(),
        },
    )
    .expect("fetch git source");
    assert_eq!(fetched.len(), 1);
    assert!(fetched[0].extract.is_some());

    let file = fs::File::open(&fetched[0].path).expect("open archive");
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let names = archive
        .entries()
        .expect("entries")
        .map(|entry| {
            entry
                .expect("entry")
                .path()
                .expect("path")
                .display()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(names.contains(&"demo/hello.txt".to_string()), "{names:?}");

    let again = fetch_sources(
        &recipe_value,
        FetchOptions {
            cache: &cache,
            proxy: None,
            offline: true,
            base_dir: work.path(),
        },
    )
    .expect("offline fetch from cache");
    assert_eq!(again[0].digest, actual);
}
