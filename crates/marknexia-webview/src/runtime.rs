//! Bundled page runtimes served by the resource broker.
//!
//! The Mermaid runtime is the file the .NET edition embeds, pinned by
//! `packaging/webview/mermaid.lock.json` and checked by `build.rs`: Mermaid
//! 10.9.1, CRLF form, the bytes .NET extracts to
//! `%LOCALAPPDATA%\Marknexia\Assets\mermaid.min.js`. Nothing is downloaded.

use std::sync::{Arc, LazyLock};

/// The only URI that loads the Mermaid runtime: the rendered page's
/// `<script src>` (.NET `TemplateEngine`), allowed by the page's
/// `script-src https://marknexia.assets` with the page nonce.
pub const MERMAID_RUNTIME_URI: &str = "https://marknexia.assets/mermaid.min.js";

/// The pinned version (`mermaid.lock.json` `version`).
pub const MERMAID_VERSION: &str = "10.9.1";

static MERMAID_RUNTIME_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mermaid.min.js"));

/// One shared copy, made on the first request, so every response streams
/// from the same buffer instead of copying 3.3 MB per request.
static MERMAID_RUNTIME: LazyLock<Arc<Vec<u8>>> =
    LazyLock::new(|| Arc::new(MERMAID_RUNTIME_BYTES.to_vec()));

/// A runtime the broker may serve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundledRuntime {
    Mermaid,
}

impl BundledRuntime {
    #[must_use]
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Mermaid => "text/javascript; charset=utf-8",
        }
    }

    /// The pinned bytes.
    #[must_use]
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Mermaid => MERMAID_RUNTIME_BYTES,
        }
    }

    pub(crate) fn shared(self) -> &'static Arc<Vec<u8>> {
        match self {
            Self::Mermaid => &MERMAID_RUNTIME,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_the_lock_file_and_the_embedded_bytes() {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../packaging/webview/mermaid.lock.json"))
                .unwrap();
        assert_eq!(lock["version"], MERMAID_VERSION);
        assert_eq!(lock["servedUri"], MERMAID_RUNTIME_URI);
        // build.rs checked the hash; the length ties the embedded bytes to it.
        assert_eq!(
            Some(MERMAID_RUNTIME_BYTES.len() as u64),
            lock["served"]["bytes"].as_u64()
        );
        assert_eq!(
            BundledRuntime::Mermaid.shared().as_slice(),
            MERMAID_RUNTIME_BYTES
        );
    }
}
