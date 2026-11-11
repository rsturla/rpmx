use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn run(root: &Path, args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .current_dir(root)
        .env("NO_PROXY", "host.containers.internal")
        .env("no_proxy", "host.containers.internal")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

fn package(root: &Path, name: &str, version: &str) {
    let recipe = root.join("package.yaml");
    let body = serde_json::json!({
        "schema": 1,
        "package": {"name": name, "version": version, "release": 1,
            "summary": "Metadata cache fixture", "license": "MIT", "source-date-epoch": 1704067200},
        "outputs": [{"name": name, "arch": "noarch", "metapackage": true}],
    });
    std::fs::write(&recipe, serde_yaml_ng::to_string(&body).unwrap()).unwrap();
    run(
        root,
        &[
            "build",
            "package.yaml",
            "--stage",
            "stage",
            "--output",
            "repository",
            "--no-tests",
        ],
    );
}

fn publish(root: &Path) {
    let path = root.join("repository").join(std::env::consts::ARCH);
    run(
        root,
        &[
            "repo",
            "--rpms",
            path.to_str().unwrap(),
            "--output",
            path.to_str().unwrap(),
        ],
    );
}

struct Server {
    port: u16,
    counts: Arc<Mutex<BTreeMap<String, usize>>>,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn new(root: PathBuf) -> Self {
        let listener = TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let counts = Arc::new(Mutex::new(BTreeMap::new()));
        let recorded = counts.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("HTTP accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                let fields: Vec<_> = request.split_whitespace().collect();
                if fields.len() < 2 {
                    continue;
                }
                let path = fields[1].split('?').next().unwrap();
                *recorded
                    .lock()
                    .unwrap()
                    .entry(path.to_string())
                    .or_default() += 1;
                let relative = Path::new(path.trim_start_matches('/'));
                let safe = relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)));
                let bytes = if safe {
                    std::fs::read(root.join(relative)).ok()
                } else {
                    None
                };
                let status = if bytes.is_some() {
                    "200 OK"
                } else {
                    "404 Not Found"
                };
                let body = bytes.unwrap_or_default();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                if fields[0] != "HEAD" {
                    let _ = stream.write_all(&body);
                }
            }
        });
        Self {
            port,
            counts,
            stopped,
            thread: Some(thread),
        }
    }

    fn requests(&self, suffix: &str) -> usize {
        self.counts
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.ends_with(suffix))
            .map(|(_, count)| count)
            .sum()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn resolve(root: &Path, server: &Server, request: &str, output: &str) -> rpmx_core::root::RootLock {
    let profile = serde_json::json!({
        "document": "rpmx-profile", "version": 1, "name": "metadata-cache-regression",
        "releasever": "44", "dist-tag": ".test", "archs": [std::env::consts::ARCH],
        "builder": {"image": "registry.fedoraproject.org/fedora:44"},
        "repositories": [{"id": "remote", "baseurl": format!("http://host.containers.internal:{}", server.port)}],
        "packages": {"install": [request]}, "options": {"weak_deps": false},
    });
    std::fs::write(
        root.join("profile.yaml"),
        serde_yaml_ng::to_string(&profile).unwrap(),
    )
    .unwrap();
    run(
        root,
        &[
            "root",
            "resolve",
            "profile.yaml",
            "--output",
            output,
            "--cache",
            "cache",
        ],
    );
    rpmx_core::root::load_lock(&root.join(output)).unwrap()
}

#[test]
#[ignore = "requires rootless podman, the Fedora 44 builder image and host.containers.internal networking"]
fn metadata_is_reused_but_republished_packages_are_seen_and_transactions_stay_separate() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("stage")).unwrap();
    package(root, "first", "1");
    package(root, "second", "1");
    publish(root);
    let server = Server::new(root.join("repository").join(std::env::consts::ARCH));
    let first = resolve(root, &server, "first", "first.lock.yaml");
    let packages: Vec<_> = first.packages.values().flatten().collect();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name, "first");
    assert_eq!(packages[0].evr, "1-1");
    let primary_requests = server.requests("primary.xml.gz");
    assert!(primary_requests > 0);
    let second = resolve(root, &server, "second", "second.lock.yaml");
    let packages: Vec<_> = second.packages.values().flatten().collect();
    assert_eq!(
        packages.len(),
        1,
        "cached metadata must not replay an earlier transaction"
    );
    assert_eq!(packages[0].name, "second");
    assert_eq!(
        server.requests("primary.xml.gz"),
        primary_requests,
        "unchanged metadata was downloaded again"
    );
    assert!(
        server.requests("repomd.xml") >= 2,
        "the warm resolution must still check repomd"
    );
    package(root, "first", "2");
    publish(root);
    let updated = resolve(root, &server, "first", "updated.lock.yaml");
    let packages: Vec<_> = updated.packages.values().flatten().collect();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name, "first");
    assert_eq!(
        packages[0].evr, "2-1",
        "new repomd must invalidate old solv/primary metadata"
    );
    assert!(server.requests("primary.xml.gz") > primary_requests);
}
