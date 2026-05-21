use super::*;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

#[tokio::test]
async fn fetch_packument_reports_url_and_body_prefix_for_invalid_json() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;

    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut request = [0; 1024];
        let _ = stream.read(&mut request);
        let body = b"not json";
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    });

    let client = RegistryClient::new(Url::parse(&format!("http://{addr}/"))?);
    let err = match client.fetch_packument(&PackageName::from("demo")).await {
        Ok(_) => anyhow::bail!("invalid JSON should fail"),
        Err(err) => err,
    };
    let message = format!("{err:#}");

    assert!(message.contains("failed to decode packument JSON"));
    assert!(message.contains(&format!("http://{addr}/demo")));
    assert!(message.contains("content-type: text/plain"));
    assert!(message.contains("not json"));
    Ok(())
}

#[test]
fn body_prefix_escapes_control_characters() {
    assert_eq!(body_prefix(b"hello\nworld"), "hello\\nworld");
}

#[tokio::test]
async fn fetch_packument_retries_body_read_eof() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let requests = Arc::new(AtomicUsize::new(0));
    let server_requests = Arc::clone(&requests);

    thread::spawn(move || {
        for attempt in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            server_requests.fetch_add(1, Ordering::SeqCst);

            let mut request = [0; 1024];
            let _ = stream.read(&mut request);

            if attempt == 0 {
                let response = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n";
                let _ = stream.write_all(response);
                continue;
            }

            let body = br#"{
                "name": "demo",
                "dist-tags": { "latest": "1.0.0" },
                "versions": {
                    "1.0.0": { "name": "demo", "version": "1.0.0" }
                }
            }"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body);
        }
    });

    let client = RegistryClient::new(Url::parse(&format!("http://{addr}/"))?);
    let packument = client.fetch_packument(&PackageName::from("demo")).await?;

    assert_eq!(packument.name, "demo");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn fetch_packument_reports_exhausted_body_read_retries() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let requests = Arc::new(AtomicUsize::new(0));
    let server_requests = Arc::clone(&requests);

    thread::spawn(move || {
        for _ in 0..PACKUMENT_MAX_RETRIES {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            server_requests.fetch_add(1, Ordering::SeqCst);

            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            let response = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n";
            let _ = stream.write_all(response);
        }
    });

    let client = RegistryClient::new(Url::parse(&format!("http://{addr}/"))?);
    let err = match client.fetch_packument(&PackageName::from("demo")).await {
        Ok(_) => anyhow::bail!("exhausted body read retries should fail"),
        Err(err) => err,
    };
    let message = format!("{err:#}");

    assert!(message.contains(&format!("after {PACKUMENT_MAX_RETRIES} attempts")));
    assert!(message.contains("unexpected EOF during chunk size line"));
    assert_eq!(requests.load(Ordering::SeqCst), PACKUMENT_MAX_RETRIES);
    Ok(())
}
