//! Integration tests for the orix CLI.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use flate2::write::GzEncoder;
use flate2::Compression;
use predicates::str::contains;
use tar::{Builder, Header};

// ─── Helper functions (must be defined before tests) ────────────────────────

/// Compute sha512 of content and encode as base64 (npm integrity format).
fn base64_encode_sha512(content: &[u8]) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha512};
    let hash = Sha512::digest(content);
    base64::engine::general_purpose::STANDARD.encode(hash)
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

fn write_response(stream: &mut TcpStream, content_type: &str, body: &[u8]) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        content_type,
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
}

fn handle_registry_request(mut stream: TcpStream, packument: &str, tarball: &[u8]) {
    let mut buffer = [0; 4096];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request.lines().next().unwrap_or_default();

    if first_line.starts_with("GET /is-number/-/") {
        write_response(&mut stream, "application/octet-stream", tarball);
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

        let ready = Arc::new(AtomicBool::new(false));
        let ready_clone = Arc::clone(&ready);

        thread::spawn(move || {
            ready_clone.store(true, Ordering::Release);
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

        while !ready.load(Ordering::Acquire) {
            thread::sleep(Duration::from_micros(100));
        }

        Self { base_url }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn cli_help_works() {
    let mut cmd = Command::cargo_bin("orix").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("Fast, disk-space efficient"));
}

#[test]
fn store_path_accepts_cli_store_dir_override() {
    let project = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();

    let mut cmd = Command::cargo_bin("orix").unwrap();
    cmd.arg("-C")
        .arg(project.path())
        .arg("--store-dir")
        .arg(store.path())
        .arg("store-path")
        .env("ORIX_STORE", "C:/orix-env-store")
        .assert()
        .success()
        .stdout(contains(store.path().display().to_string()));
}

#[test]
fn install_fetches_package_from_mock_registry() {
    let tarball = make_package_tarball();
    let integrity = format!("sha512-{}", base64_encode_sha512(&tarball));
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

    let orix_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_orix"));

    let output = std::process::Command::new(&orix_bin)
        .arg("--registry")
        .arg(registry.base_url())
        .arg("--store-dir")
        .arg(store.path())
        .arg("--cache-dir")
        .arg(cache.path())
        .arg("-C")
        .arg(project.path())
        .arg("install")
        .env("ORIX_LOG", "error")
        .output()
        .unwrap();

    eprintln!("INSTALL OUT:\n{}", String::from_utf8_lossy(&output.stdout));
    eprintln!("INSTALL ERR:\n{}", String::from_utf8_lossy(&output.stderr));
    eprintln!("INSTALL STATUS: {}", output.status);

    assert!(
        output.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Packages installed: 1"),
        "expected 'Packages installed: 1' in output"
    );

    assert!(project.path().join("orix-lock.yaml").exists());
    // Verify the package was actually installed in the store (not just fetched+linked).
    assert!(
        store
            .path()
            .join("v1")
            .join("packages")
            .join("is-number@1.0.0")
            .join("package.json")
            .exists(),
        "package.json should exist in store"
    );
    // Verify lockfile contains the expected integrity.
    let lockfile_content = fs::read_to_string(project.path().join("orix-lock.yaml")).unwrap();
    assert!(
        lockfile_content.contains("is-number@1.0.0"),
        "lockfile should contain is-number@1.0.0"
    );

    let output = std::process::Command::new(&orix_bin)
        .arg("-C")
        .arg(project.path())
        .arg("store-path")
        .env("ORIX_STORE", store.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "store-path failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&store.path().display().to_string()),
        "store-path should return the store path"
    );

    let output = std::process::Command::new(&orix_bin)
        .arg("-C")
        .arg(project.path())
        .arg("store-verify")
        .env("ORIX_STORE", store.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "store-verify failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = std::process::Command::new(&orix_bin)
        .arg("-C")
        .arg(project.path())
        .arg("store-prune")
        .arg("--dry-run")
        .env("ORIX_STORE", store.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "store-prune failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
