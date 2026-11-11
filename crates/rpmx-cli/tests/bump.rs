use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};

const RECIPE: &str = r#"
schema: 1
package:
  name: probe
  version: "1.0"
  release: '7' # packaging change
  summary: A probe package
  license: MIT
build:
  pipeline:
    - runs: "true"
sources:
  - name: probe.tar.gz
    kind: archive
    digest: "sha512:00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    locators:
      - kind: https
        url: https://example.invalid/probe-1.0.tar.gz
outputs:
  - name: probe
    metapackage: true
"#;

fn rpmx(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .expect("run rpmx")
}

#[test]
fn bump_updates_the_version_and_source_urls() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    std::fs::write(&recipe, RECIPE).expect("recipe");
    let output = rpmx(&["bump", "1.1", recipe.to_str().unwrap(), "--no-fetch"]);
    assert!(output.status.success(), "{output:?}");
    let text = std::fs::read_to_string(&recipe).expect("read");
    assert!(text.contains("version: \"1.1\""), "{text}");
    assert!(text.contains("probe-1.1.tar.gz"), "{text}");
    assert!(text.contains("release: '1' # packaging change"), "{text}");
}

#[test]
fn bump_dry_run_does_not_write() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    std::fs::write(&recipe, RECIPE).expect("recipe");
    let output = rpmx(&[
        "bump",
        "1.1",
        recipe.to_str().unwrap(),
        "--no-fetch",
        "--dry-run",
    ]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("+  version: \"1.1\""), "{text}");
    assert!(
        text.contains("+  release: '1' # packaging change"),
        "{text}"
    );
    assert!(std::fs::read_to_string(&recipe).unwrap().contains("1.0"));
}

/// Serve distinct bytes for the old and new URLs, then return the request.
fn release_server(status: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let status = status.to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .expect("timeout");
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).expect("request");
            assert_ne!(count, 0, "incomplete request");
            request.extend_from_slice(&buffer[..count]);
        }
        let request = String::from_utf8(request).expect("HTTP request");
        let body = if request.starts_with("GET /probe-1.1.tar.gz ") {
            "new release bytes"
        } else {
            "old release bytes"
        };
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("response");
        request
    });
    (format!("http://{address}"), server)
}

fn network_recipe(base: &str) -> String {
    RECIPE.replace("https://example.invalid", base)
}

#[test]
fn bump_hashes_the_new_release_bytes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    let (base, server) = release_server("200 OK");
    std::fs::write(&recipe, network_recipe(&base)).expect("recipe");
    let output = rpmx(&["bump", "1.1", recipe.to_str().unwrap()]);
    let request = server.join().expect("server");
    assert!(output.status.success(), "{output:?}");
    assert!(request.starts_with("GET /probe-1.1.tar.gz "), "{request}");
    let updated = rpmx_core::load_recipe_bundle(&recipe).expect("updated recipe");
    assert_eq!(updated.recipe.package.release, "1");
    assert_eq!(
        updated.recipe.sources[0].digest,
        rpmx_core::srpm::sha512_hex(b"new release bytes")
    );
}

#[test]
fn fetching_during_dry_run_does_not_write_the_recipe() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    let (base, server) = release_server("200 OK");
    let original = network_recipe(&base);
    std::fs::write(&recipe, &original).expect("recipe");
    let output = rpmx(&["bump", "1.1", recipe.to_str().unwrap(), "--dry-run"]);
    server.join().expect("server");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(std::fs::read_to_string(recipe).unwrap(), original);
    let diff = String::from_utf8_lossy(&output.stdout);
    assert!(
        diff.contains(&rpmx_core::srpm::sha512_hex(b"new release bytes")),
        "{diff}"
    );
}

#[test]
fn a_missing_new_release_leaves_the_recipe_unchanged() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    let (base, server) = release_server("404 Not Found");
    let original = network_recipe(&base);
    std::fs::write(&recipe, &original).expect("recipe");
    let output = rpmx(&["bump", "1.1", recipe.to_str().unwrap()]);
    let request = server.join().expect("server");
    assert!(!output.status.success(), "{output:?}");
    assert!(request.starts_with("GET /probe-1.1.tar.gz "), "{request}");
    assert_eq!(std::fs::read_to_string(recipe).unwrap(), original);
}

#[test]
fn bump_refreshes_a_split_source_file_and_keeps_relative_file_sources() {
    let directory = tempfile::tempdir().expect("tempdir");
    let recipe = directory.path().join("package.yaml");
    let (base, server) = release_server("200 OK");
    let original = network_recipe(&base);
    let (package, rest) = original.split_once("sources:\n").unwrap();
    let (sources, outputs) = rest.split_once("outputs:\n").unwrap();
    std::fs::write(
        &recipe,
        format!("{package}include: [sources.yaml]\noutputs:\n{outputs}"),
    )
    .expect("recipe");
    let source_path = directory.path().join("sources.yaml");
    std::fs::write(directory.path().join("extra.txt"), "extra source bytes").expect("file source");
    std::fs::write(&source_path, format!("sources:\n{sources}  - name: extra\n    kind: archive\n    digest: sha512:{}\n    locators:\n      - kind: file\n        path: extra.txt\n", "0".repeat(128))).expect("sources");
    let output = rpmx(&["bump", "1.1", recipe.to_str().unwrap()]);
    server.join().expect("server");
    assert!(output.status.success(), "{output:?}");
    let updated = rpmx_core::load_recipe_bundle(&recipe).expect("updated recipe");
    assert_eq!(updated.recipe.package.version, "1.1");
    assert_eq!(updated.recipe.package.release, "1");
    assert_eq!(
        updated.recipe.sources[0].digest,
        rpmx_core::srpm::sha512_hex(b"new release bytes")
    );
    assert_eq!(
        updated.recipe.sources[1].digest,
        rpmx_core::srpm::sha512_hex(b"extra source bytes")
    );
}
