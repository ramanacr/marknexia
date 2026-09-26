//! The .NET `TemplateEngine.GenerateHtml` page, with `\n` line endings (the
//! frozen fixtures are newline-normalized; REND-3). The page is trusted
//! template text around the sanitized body: bundled CSS and bridge script
//! copied byte-for-byte from `src/Marknexia.Rendering/Assets` at the fixture
//! revision, plus entity-encoded context values.

use marknexia_core::contracts::AppTheme;

use crate::text::{escape_data_string, html_encode};

pub(crate) const GITHUB_MARKDOWN_CSS: &str = include_str!("../assets/github-markdown.css");
pub(crate) const BRIDGE_JS: &str = include_str!("../assets/bridge.js");

/// Per-document secrets the .NET template draws from `Guid.NewGuid()` and
/// `RandomNumberGenerator`. The host generates them from the OS RNG and passes
/// them in, so this crate stays deterministic and free of an entropy
/// dependency. `Debug` never prints the nonce.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PageIdentity {
    document_id: [u8; 16],
    nonce: [u8; 16],
}

/// A [`PageIdentity`] value that cannot be a fresh random secret.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidPageIdentity {
    ZeroDocumentId,
    ZeroNonce,
}

impl std::fmt::Display for InvalidPageIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ZeroDocumentId => "page document id is all zero",
            Self::ZeroNonce => "page nonce is all zero",
        })
    }
}

impl std::error::Error for InvalidPageIdentity {}

impl std::fmt::Debug for PageIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageIdentity")
            .field("origin", &self.origin())
            .field("nonce", &"<redacted>")
            .finish()
    }
}

impl PageIdentity {
    /// `document_id` becomes the 32-hex-digit host label
    /// `document-{id}.marknexia.viewer`; `nonce` becomes the base64 CSP nonce.
    /// Both must be fresh OS-RNG bytes per rendered document; all-zero values
    /// (an unfilled buffer) are rejected.
    pub fn new(document_id: [u8; 16], nonce: [u8; 16]) -> Result<Self, InvalidPageIdentity> {
        if document_id == [0; 16] {
            return Err(InvalidPageIdentity::ZeroDocumentId);
        }
        if nonce == [0; 16] {
            return Err(InvalidPageIdentity::ZeroNonce);
        }
        Ok(Self { document_id, nonce })
    }

    /// `https://document-{hex}.marknexia.viewer`.
    #[must_use]
    pub fn origin(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut origin = String::with_capacity(64);
        origin.push_str("https://document-");
        for byte in self.document_id {
            origin.push(char::from(HEX[usize::from(byte >> 4)]));
            origin.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
        origin.push_str(".marknexia.viewer");
        origin
    }

    /// `Convert.ToBase64String(nonce)`.
    #[must_use]
    pub fn nonce(&self) -> String {
        base64(&self.nonce)
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0_u32, |acc, (index, &byte)| {
            acc | u32::from(byte) << (16 - 8 * index)
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[(value >> (18 - 6 * index) & 0x3F) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Where page bytes go: a string, or a byte counter used to enforce the
/// output limit before any page is built.
pub(crate) trait Sink {
    fn put(&mut self, text: &str);
}

impl Sink for String {
    fn put(&mut self, text: &str) {
        self.push_str(text);
    }
}

pub(crate) struct Counter(pub(crate) usize);

impl Sink for Counter {
    fn put(&mut self, text: &str) {
        self.0 = self.0.saturating_add(text.len());
    }
}

/// Everything in the page except the body, already encoded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PageHead {
    theme: &'static str,
    base_href: String,
    content_security_policy: String,
    nonce: String,
    mermaid_script: bool,
}

impl PageHead {
    pub(crate) fn new(
        theme: AppTheme,
        allow_remote_assets: bool,
        directory_segments: &[&str],
        identity: &PageIdentity,
        mermaid_script: bool,
    ) -> Self {
        let theme = match theme {
            AppTheme::Dark => "dark",
            AppTheme::Light => "light",
            AppTheme::System | AppTheme::HighContrast => "system",
        };
        let origin = identity.origin();
        let nonce = identity.nonce();
        let mut base = origin.clone();
        base.push('/');
        for segment in directory_segments {
            escape_data_string(segment, &mut base);
            base.push('/');
        }
        let remote = if allow_remote_assets {
            " http: https:"
        } else {
            ""
        };
        let policy = [
            "default-src 'none'".to_owned(),
            format!("base-uri {origin}"),
            format!("script-src 'nonce-{nonce}' https://marknexia.assets"),
            "style-src 'unsafe-inline'".to_owned(),
            format!("img-src {origin} data:{remote}"),
            "font-src 'none'".to_owned(),
            "object-src 'none'".to_owned(),
            "frame-src 'none'".to_owned(),
            "connect-src 'none'".to_owned(),
            "form-action 'none'".to_owned(),
        ]
        .join("; ");
        let mut base_href = String::new();
        html_encode(&base, &mut base_href);
        let mut content_security_policy = String::new();
        html_encode(&policy, &mut content_security_policy);
        Self {
            theme,
            base_href,
            content_security_policy,
            nonce,
            mermaid_script,
        }
    }

    pub(crate) fn write(&self, body: &str, sink: &mut impl Sink) {
        sink.put("<!DOCTYPE html>\n<html lang=\"en\" data-theme=\"");
        sink.put(self.theme);
        sink.put("\">\n<head>\n  <meta charset=\"utf-8\" />\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\" />\n  <base href=\"");
        sink.put(&self.base_href);
        sink.put("\" />\n  <meta http-equiv=\"Content-Security-Policy\" content=\"");
        sink.put(&self.content_security_policy);
        sink.put("\" />\n  <style>\n");
        sink.put(GITHUB_MARKDOWN_CSS);
        sink.put("\n  </style>\n</head>\n<body>\n  <div class=\"markdown-body\">\n");
        sink.put(body);
        sink.put("\n  </div>\n");
        if self.mermaid_script {
            sink.put("  <script src=\"https://marknexia.assets/mermaid.min.js\" nonce=\"");
            sink.put(&self.nonce);
            sink.put("\"></script>\n");
        }
        sink.put("  <script nonce=\"");
        sink.put(&self.nonce);
        sink.put("\">\n");
        sink.put(BRIDGE_JS);
        sink.put("\n  </script>\n</body>\n</html>\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_formats_like_dotnet() {
        let identity = PageIdentity::new([0xAB; 16], [0xFF; 16]).unwrap();
        assert!(!format!("{identity:?}").contains(&identity.nonce()));
        assert!(format!("{identity:?}").contains("<redacted>"));
        assert_eq!(
            PageIdentity::new([0; 16], [1; 16]),
            Err(InvalidPageIdentity::ZeroDocumentId)
        );
        assert_eq!(
            PageIdentity::new([1; 16], [0; 16]),
            Err(InvalidPageIdentity::ZeroNonce)
        );
        assert_eq!(
            identity.origin(),
            "https://document-abababababababababababababababab.marknexia.viewer"
        );
        assert_eq!(identity.nonce(), "/////////////////////w==");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn counter_matches_written_length() {
        let identity = PageIdentity::new([1; 16], [2; 16]).unwrap();
        for mermaid in [false, true] {
            let head = PageHead::new(AppTheme::Dark, true, &["a b", "c"], &identity, mermaid);
            let mut page = String::new();
            head.write("<p>x</p>", &mut page);
            let mut counter = Counter(0);
            head.write("<p>x</p>", &mut counter);
            assert_eq!(page.len(), counter.0);
            assert!(page.contains("<base href=\"https://document-"));
            assert!(page.contains(".marknexia.viewer/a%20b/c/\" />"));
        }
    }
}
