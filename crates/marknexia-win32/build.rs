//! Embeds an application manifest that selects Common Controls v6
//! (comctl32 6.0) for the shell executable: the sidebar's TreeView, tab and
//! list controls, visual styles, and their UI Automation providers come
//! from that version. Only the binary gets it (`rustc-link-arg-bins`);
//! library tests never create controls.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if windows && msvc {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    } else if windows {
        // Without the manifest the shell runs on comctl32 5.8: no visual
        // styles, and the sidebar controls lose their v6 behaviour.
        println!(
            "cargo:warning=marknexia-win32: no Common Controls 6.0 manifest is embedded for this non-MSVC Windows target"
        );
    }
}
