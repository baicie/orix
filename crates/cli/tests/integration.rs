//! Integration tests for the orix CLI.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use flate2::write::GzEncoder;
use flate2::Compression;
use predicates::str::contains;
use sha2::{Digest, Sha512};
use tar::{Builder, Header};

#[test]
fn cli_help_works() {
    let mut cmd = Command::cargo_bin("orix").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("Fast, disk-space efficient"));
}

#[test]
fn install_fetches_package_from_mock_registry() {
    let tarball = make_package_tarball();
    let integrity = format!("sha512-{}", hex::encode(Sha512::digest(&tarball)));
    let registry = MockRegistry::start(tarball, integrity.clone());

    let project = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("package.json"),
        r#"{
  "name": "fixture-app",
  "version": "1.0.0",
  "dependencies": {
    "is-number": "1.0.0"
  }
}"#,
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("orix").unwrap();
    cmd.arg("--registry")
        .arg(registry.base_url())
        .arg("-C")
        .arg(project.path())
        .arg("install")
        .env("ORIX_STORE", store.path())
        .env("ORIX_CACHE", cache.path())
        .env("ORIX_LOG", "error")
        .assert()
        .success()
        .stdout(contains("Packages installed: 1"));

    assert!(project.path().join("orix-lock.yaml").exists());
    assert!(project
        .path()
        .join("node_modules")
        .join("is-number")
        .join("package.json")
        .exists());

    let mut path_cmd = Command::cargo_bin("orix").unwrap();
    path_cmd
        .arg("-C")
        .arg(project.path())
        .arg("store-path")
        .env("ORIX_STORE", store.path())
        .assert()
        .success()
        .stdout(contains(store.path().display().to_string()));

    let mut verify_cmd = Command::cargo_bin("orix").unwrap();
    verify_cmd
        .arg("-C")
        .arg(project.path())
        .arg("store-verify")
        .env("ORIX_STORE", store.path())
        .assert()
        .success()
        .stdout(contains("Store verified"));

    let mut prune_cmd = Command::cargo_bin("orix").unwrap();
    prune_cmd
        .arg("-C")
        .arg(project.path())
        .arg("store-prune")
        .arg("--dry-run")
        .env("ORIX_STORE", store.path())
        .assert()
        .success()
        .stdout(contains("Would remove 0 packages"));
}

struct MockRegistry {
    base_url: String,
}

impl MockRegistry {
    fn start(tarball: Vec<u8>, integrity: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{}/", addr);
        let tarball_url = format!("{}is-number/-/is-number-1.0.0.tgz", base_url);
        let packument = format!(
            r#"{{
  "name": "is-number",
  "versions": {{
    "1.0.0": {{
      "name": "is-number",
      "version": "1.0.0",
      "dist": {{
        "tarball": "{}",
        "integrity": "{}"
      }}
    }}
  }}
}}"#,
            tarball_url, integrity
        );

        // Signal that the listener is bound and ready before the thread starts accepting.
        let ready = Arc::new(AtomicBool::new(false));
        let ready_clone = Arc::clone(&ready);
        let deadline = Arc::new(AtomicU64::new(0));

        thread::spawn(move || {
            ready_clone.store(true, Ordering::Release);
            let deadline_val = Instant::now() + Duration::from_secs(30);
            deadline.store(
                deadline_val
                    .duration_since(Instant::now())
                    .as_secs()
                    .try_into()
                    .unwrap_or(30),
                Ordering::Relaxed,
            );
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        handle_registry_request(stream, &packument, &tarball);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                        break;
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        });

        // Wait for the server thread to be ready to accept connections.
        while !ready.load(Ordering::Acquire) {
            thread::sleep(Duration::from_micros(100));
        }

        Self { base_url }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }
}

fn handle_registry_request(mut stream: TcpStream, packument: &str, tarball: &[u8]) {
    let mut buffer = [0; 4096];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request.lines().next().unwrap_or_default();

    // Tarball: GET /is-number/-/is-number-1.0.0.tgz (check before packument due to prefix match)
    if first_line.starts_with("GET /is-number/-/") {
        write_response(&mut stream, "application/octet-stream", tarball);
    // Packument: exact match on /is-number or /is-number/
    } else if first_line.starts_with("GET /is-number ") || first_line.starts_with("GET /is-number/")
    {
        write_response(&mut stream, "application/json", packument.as_bytes());
    } else {
        let body = b"not found";
        let response = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
    }
}

fn write_response(stream: &mut TcpStream, content_type: &str, body: &[u8]) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        content_type,
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
}

fn make_package_tarball() -> Vec<u8> {
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut archive = Builder::new(&mut gzip);
        append_file(
            &mut archive,
            "package/package.json",
            br#"{"name":"is-number","version":"1.0.0","main":"index.js"}"#,
        );
        append_file(
            &mut archive,
            "package/index.js",
            b"module.exports = value => typeof value === 'number';\n",
        );
        archive.finish().unwrap();
    }
    gzip.finish().unwrap()
}

fn append_file(archive: &mut Builder<&mut GzEncoder<Vec<u8>>>, path: &str, content: &[u8]) {
    let mut header = Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive.append_data(&mut header, path, content).unwrap();
}
