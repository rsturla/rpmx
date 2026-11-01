use std::io::{Read, Write};
use std::net::TcpListener;

use rpmx_core::fetch::{fetch_sources, FetchOptions};
use rpmx_core::{parse_recipe, srpm::sha512_hex};

fn recipe(digest: &str, locator: &str) -> String {
    format!(
        r#"
schema: 1
package:
  name: demo
  version: "1.0"
  release: 1
  summary: demo
  license: MIT
sources:
  - name: demo
    kind: archive
    digest: "{digest}"
    locators:
{locator}
outputs:
  - name: demo
    files:
      include: ["/usr/bin/**"]
"#
    )
}

/// Serve one HTTP GET with `body`, returning the bound port and server thread.
fn one_shot_http(body: Vec<u8>) -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("addr").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("headers");
        stream.write_all(&body).expect("body");
    });
    (port, server)
}

#[test]
fn file_locator_rejects_a_non_regular_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("source.txt")).expect("directory");
    let yaml = recipe(
        &format!("sha512:{}", "0".repeat(128)),
        "      - kind: file\n        path: source.txt",
    );
    let recipe = parse_recipe(&yaml).expect("recipe");
    let cache = dir.path().join("cache");

    let error = fetch_sources(
        &recipe,
        FetchOptions {
            cache: &cache,
            proxy: None,
            offline: false,
            base_dir: dir.path(),
        },
    )
    .expect_err("directory locator");
    assert_eq!(error.code, "RPMX-FETCH-005");
    assert!(error.message.contains("regular file"), "{}", error.message);
}

#[test]
fn file_locator_populates_the_cache() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"source bytes";
    std::fs::write(dir.path().join("source.txt"), bytes).expect("source");
    let digest = sha512_hex(bytes);
    let yaml = recipe(&digest, "      - kind: file\n        path: source.txt");
    let recipe = parse_recipe(&yaml).expect("recipe");
    let cache = dir.path().join("cache");

    let fetched = fetch_sources(
        &recipe,
        FetchOptions {
            cache: &cache,
            proxy: None,
            offline: false,
            base_dir: dir.path(),
        },
    )
    .expect("fetch");

    assert_eq!(fetched.len(), 1);
    assert_eq!(
        std::fs::read(&fetched[0].path).expect("cached"),
        b"source bytes"
    );
    assert!(fetched[0].path.starts_with(cache.join("sha512")));
}

#[test]
fn digest_mismatch_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("source.txt"), b"tampered").expect("source");
    let digest = sha512_hex(b"expected");
    let yaml = recipe(&digest, "      - kind: file\n        path: source.txt");
    let recipe = parse_recipe(&yaml).expect("recipe");

    let error = fetch_sources(
        &recipe,
        FetchOptions {
            cache: &dir.path().join("cache"),
            proxy: None,
            offline: false,
            base_dir: dir.path(),
        },
    )
    .expect_err("mismatch");
    assert_eq!(error.code, "RPMX-FETCH-004");
}

#[test]
fn offline_fails_on_a_cache_miss() {
    let dir = tempfile::tempdir().expect("tempdir");
    let digest = sha512_hex(b"source bytes");
    let yaml = recipe(&digest, "      - kind: file\n        path: source.txt");
    let recipe = parse_recipe(&yaml).expect("recipe");

    let error = fetch_sources(
        &recipe,
        FetchOptions {
            cache: &dir.path().join("cache"),
            proxy: None,
            offline: true,
            base_dir: dir.path(),
        },
    )
    .expect_err("offline miss");
    assert_eq!(error.code, "RPMX-FETCH-006");
}

#[test]
fn https_locator_downloads_and_verifies() {
    let bytes = b"http source".to_vec();
    let digest = sha512_hex(&bytes);
    let (port, server) = one_shot_http(bytes.clone());

    let dir = tempfile::tempdir().expect("tempdir");
    let yaml = recipe(
        &digest,
        &format!("      - kind: https\n        url: http://127.0.0.1:{port}/source"),
    );
    let recipe = parse_recipe(&yaml).expect("recipe");
    let fetched = fetch_sources(
        &recipe,
        FetchOptions {
            cache: &dir.path().join("cache"),
            proxy: None,
            offline: false,
            base_dir: dir.path(),
        },
    )
    .expect("fetch");
    server.join().expect("server");
    assert_eq!(
        std::fs::read(&fetched[0].path).expect("cached"),
        b"http source"
    );
}

#[test]
fn content_addressed_proxy_is_used_first() {
    let bytes = b"proxied source".to_vec();
    let digest = sha512_hex(&bytes);
    let (port, server) = one_shot_http(bytes.clone());

    let dir = tempfile::tempdir().expect("tempdir");
    let yaml = recipe(&digest, "      - kind: file\n        path: missing");
    let recipe = parse_recipe(&yaml).expect("recipe");
    let proxy = format!("http://127.0.0.1:{port}");
    let fetched = fetch_sources(
        &recipe,
        FetchOptions {
            cache: &dir.path().join("cache"),
            proxy: Some(&proxy),
            offline: false,
            base_dir: dir.path(),
        },
    )
    .expect("fetch via proxy");
    server.join().expect("server");
    assert_eq!(
        std::fs::read(&fetched[0].path).expect("cached"),
        b"proxied source"
    );
}

#[test]
fn a_bad_locator_falls_through_to_the_next() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("bad.txt"), b"wrong").expect("bad");
    std::fs::write(dir.path().join("good.txt"), b"right").expect("good");
    let digest = sha512_hex(b"right");
    let yaml = recipe(
        &digest,
        "      - kind: file\n        path: bad.txt\n      - kind: file\n        path: good.txt",
    );
    let recipe = parse_recipe(&yaml).expect("recipe");
    let fetched = fetch_sources(
        &recipe,
        FetchOptions {
            cache: &dir.path().join("cache"),
            proxy: None,
            offline: false,
            base_dir: dir.path(),
        },
    )
    .expect("fallback");
    assert_eq!(std::fs::read(&fetched[0].path).expect("cached"), b"right");
}
