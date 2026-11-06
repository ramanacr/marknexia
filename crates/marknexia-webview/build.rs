//! Embeds the Mermaid runtime that the .NET edition ships
//! (`src/Marknexia.Rendering/Assets/mermaid.min.js`), pinned by
//! `packaging/webview/mermaid.lock.json`. Nothing is downloaded.
//!
//! The working copy may hold the file with LF or CRLF line endings (Git
//! `core.autocrlf`). The build normalizes it to LF and checks the repository
//! pin, then converts it to CRLF and checks the served pin: the bytes the
//! .NET Windows release embeds and extracts. Any drift fails the build.

use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../..");
    let lock_path = root.join("packaging/webview/mermaid.lock.json");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", lock_path.display());

    let lock: serde_json::Value = serde_json::from_slice(
        &fs::read(&lock_path).unwrap_or_else(|error| panic!("{}: {error}", lock_path.display())),
    )
    .expect("mermaid.lock.json is JSON");
    assert_eq!(lock["schemaVersion"], "bundled-runtime-lock-v1");
    assert_eq!(lock["servedUri"], "https://marknexia.assets/mermaid.min.js");
    let source = lock["source"].as_str().expect("lock source");
    let source_path = root.join(source);
    println!("cargo:rerun-if-changed={}", source_path.display());
    let raw = fs::read(&source_path).unwrap_or_else(|error| {
        panic!(
            "the pinned Mermaid runtime {} cannot be read ({error})",
            source_path.display()
        )
    });

    self_test();
    let repository = to_lf(&raw);
    check(&lock["repository"], "lf", &repository);
    let served = to_crlf(&repository);
    check(&lock["served"], "crlf", &served);

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("mermaid.min.js");
    fs::write(&out, &served).unwrap_or_else(|error| panic!("{}: {error}", out.display()));
}

fn check(pin: &serde_json::Value, line_endings: &str, bytes: &[u8]) {
    assert_eq!(pin["lineEndings"], line_endings);
    let expected_bytes = pin["bytes"].as_u64().expect("pinned byte count");
    let expected_hash = pin["sha256"].as_str().expect("pinned sha256");
    let actual_hash = hex(&sha256(bytes));
    assert!(
        bytes.len() as u64 == expected_bytes && actual_hash == expected_hash,
        "Mermaid runtime ({line_endings}) drifted from mermaid.lock.json: {} bytes, sha256 {actual_hash}; pinned {expected_bytes} bytes, sha256 {expected_hash}",
        bytes.len()
    );
}

/// CRLF to LF. A lone CR is kept, so a file that was never CRLF-converted
/// fails the hash check instead of being silently changed.
fn to_lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            index += 1;
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

fn to_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 512);
    for &byte in bytes {
        if byte == b'\n' {
            out.push(b'\r');
        }
        out.push(byte);
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// FIPS 180-4 SHA-256. A build script cannot use a dependency that the
/// workspace does not already vet, and the check is self-tested below on
/// every build.
fn sha256(message: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((message.len() as u64) * 8).to_be_bytes());
    for block in padded.chunks_exact(64) {
        let mut w = [0_u32; 64];
        for (index, word) in block.chunks_exact(4).enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut digest = [0_u8; 32];
    for (chunk, word) in digest.chunks_exact_mut(4).zip(state) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// Known answers from FIPS 180-2 appendix B (one block, two blocks) and the
/// empty message, checked on every build so a broken digest cannot accept a
/// drifted runtime.
fn self_test() {
    for (message, expected) in [
        (
            &b""[..],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
    ] {
        assert_eq!(hex(&sha256(message)), expected, "SHA-256 self-test failed");
    }
}
