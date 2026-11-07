//! 1. Embeds an application manifest with `asInvoker`, so the per-user
//!    setup never asks for elevation (and installer detection never
//!    applies), plus Common Controls 6 for the message boxes.
//! 2. Writes the payload container to `OUT_DIR/payload.bin`.
//!
//! The payload comes only from `MARKNEXIA_SETUP_PAYLOAD_DIR`, which CI sets
//! to a folder holding exactly the payload files (`EXPECTED_FILES`). Without
//! it the container is empty and the setup refuses to install
//! ("This setup executable does not contain an application payload.").
//! Developer machines never set it: the payload holds a real executable,
//! and installers are built only on CI runners.

use std::path::PathBuf;

#[allow(dead_code)]
#[path = "src/container.rs"]
mod container;

/// Keep in step with `names::RELEASE_PAYLOAD`.
const EXPECTED_FILES: [&str; 5] = [
    "README.txt",
    "THIRD-PARTY-NOTICES.txt",
    "WebView2Loader.dll",
    "marknexia-sbom.spdx.json",
    "marknexia-win32.exe",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/container.rs");
    println!("cargo:rerun-if-env-changed=MARKNEXIA_SETUP_PAYLOAD_DIR");

    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if windows && msvc {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let container = match std::env::var_os("MARKNEXIA_SETUP_PAYLOAD_DIR") {
        None => Vec::new(),
        Some(directory) => build_container(&PathBuf::from(directory)),
    };
    std::fs::write(out.join("payload.bin"), container).expect("write payload.bin");
}

fn build_container(directory: &std::path::Path) -> Vec<u8> {
    assert!(
        directory.is_absolute(),
        "MARKNEXIA_SETUP_PAYLOAD_DIR must be absolute"
    );
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .expect("read MARKNEXIA_SETUP_PAYLOAD_DIR")
        .map(|entry| {
            let entry = entry.expect("payload entry");
            assert!(
                entry.file_type().expect("payload entry type").is_file(),
                "the payload folder must hold only files"
            );
            entry
                .file_name()
                .into_string()
                .expect("payload names are UTF-8")
        })
        .collect();
    names.sort();
    let mut expected: Vec<String> = EXPECTED_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    expected.sort();
    assert_eq!(
        names, expected,
        "the payload folder must hold exactly the expected files"
    );
    let files: Vec<(String, Vec<u8>)> = names
        .into_iter()
        .map(|name| {
            let path = directory.join(&name);
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = std::fs::read(&path).expect("read payload file");
            (name, bytes)
        })
        .collect();
    container::encode(&files).expect("encode the payload container")
}
