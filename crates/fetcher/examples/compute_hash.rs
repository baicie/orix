#![allow(clippy::expect_used, missing_docs)]

fn main() {
    let path = "crates/fetcher/test-fixtures/left-pad-1.3.0.tgz";
    let data = std::fs::read(path).expect("failed to read file");
    eprintln!("File size: {} bytes", data.len());

    use sha2::{Digest, Sha256, Sha512};

    let sha512_hash = Sha512::digest(&data);
    let sha256_hash = Sha256::digest(&data);

    println!("sha512-{}", base64_encode(&sha512_hash));
    println!("sha256-{}", base64_encode(&sha256_hash));
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in input.chunks(3) {
        let b: [u8; 3] = match chunk.len() {
            1 => [chunk[0], 0, 0],
            2 => [chunk[0], chunk[1], 0],
            _ => [chunk[0], chunk[1], chunk[2]],
        };
        result.push(ALPHABET[(b[0] >> 2) as usize] as char);
        result.push(ALPHABET[(((b[0] << 4) | (b[1] >> 4)) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(ALPHABET[(((b[1] << 2) | (b[2] >> 6)) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(ALPHABET[(b[2] & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}
