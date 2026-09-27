#![forbid(unsafe_code)]

//! Security-owned content boundaries.
//!
//! [`SanitizedFragment`] and [`SanitizedSvg`] cannot be constructed outside
//! this crate. Markup crosses the boundary only through the parser-backed
//! sanitizer in [`HtmlPolicy::sanitize_fragment`] and [`SvgPolicy::sanitize`]
//! (ammonia/html5ever with the .NET-equivalent allowlists) or as plain text
//! through [`HtmlPolicy::encode_text`]. Inline CSS is decided on `cssparser`
//! tokens. No security decision is made with string replacement on markup.
//!
//! Large fragments are cut into chunks at points where the HTML parser is
//! provably back at its initial state and cleaned in parallel; the output is
//! byte-identical to cleaning the whole fragment at once (see `depth` and
//! `compat/decisions/sanitizer-ammonia-differences.md`, SAN-8 and SAN-10).
//!
//! This crate needs ammonia's DOM (`Document::to_dom_node`), which ammonia
//! only exposes under `--cfg ammonia_unstable`. The workspace
//! `.cargo/config.toml` sets that flag; a build that replaces the configured
//! rustflags (for example with `RUSTFLAGS`) must pass it too.

#[cfg(not(ammonia_unstable))]
compile_error!(
    "marknexia-security needs `--cfg ammonia_unstable` (set in the workspace .cargo/config.toml; \
     add it to RUSTFLAGS when overriding rustflags)"
);

mod attrs;
mod css;
mod depth;
mod html;
mod serialize;

pub use attrs::MAX_ATTRIBUTES_PER_TAG;
pub use depth::MAX_NESTING_DEPTH;

use std::{error::Error, fmt};

use serde::Serialize;

/// Largest HTML fragment accepted: the .NET `MarkdownRenderer.MaxRenderedHtmlBytes`
/// (128 MiB), so a rendered Markdown body up to the .NET page limit fits.
pub const MAX_HTML_INPUT_BYTES: usize = 128 * 1024 * 1024;
/// Largest sanitized output: also the .NET page limit.
pub const MAX_HTML_OUTPUT_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_SVG_INPUT_BYTES: usize = 512 * 1024;
pub const MAX_CSS_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_URL_BYTES: usize = 8 * 1024;

/// Explicit byte budgets applied before and during each policy operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PolicyLimits {
    pub max_html_input_bytes: usize,
    pub max_html_output_bytes: usize,
    pub max_svg_input_bytes: usize,
    pub max_css_input_bytes: usize,
    pub max_url_bytes: usize,
}

impl Default for PolicyLimits {
    fn default() -> Self {
        Self {
            max_html_input_bytes: MAX_HTML_INPUT_BYTES,
            max_html_output_bytes: MAX_HTML_OUTPUT_BYTES,
            max_svg_input_bytes: MAX_SVG_INPUT_BYTES,
            max_css_input_bytes: MAX_CSS_INPUT_BYTES,
            max_url_bytes: MAX_URL_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteImagePolicy {
    /// Remote image sources are removed.
    Deny,
    /// Valid `https://` image sources are kept.
    AllowHttps,
    /// A remote `<img src>` (a value starting with `http://`, `https://` or
    /// `//`, in any case) becomes `src=""`, as the .NET renderer's
    /// `BlockRemoteImages` leaves it; every other remote image source is
    /// removed as under [`Self::Deny`].
    Blank,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentPolicy {
    limits: PolicyLimits,
    remote_images: RemoteImagePolicy,
}

impl ContentPolicy {
    pub fn try_new(
        limits: PolicyLimits,
        remote_images: RemoteImagePolicy,
    ) -> Result<Self, SanitizeError> {
        for (kind, requested, cap) in [
            (
                "HTML input",
                limits.max_html_input_bytes,
                MAX_HTML_INPUT_BYTES,
            ),
            (
                "HTML output",
                limits.max_html_output_bytes,
                MAX_HTML_OUTPUT_BYTES,
            ),
            ("SVG input", limits.max_svg_input_bytes, MAX_SVG_INPUT_BYTES),
            ("CSS input", limits.max_css_input_bytes, MAX_CSS_INPUT_BYTES),
            ("URL", limits.max_url_bytes, MAX_URL_BYTES),
        ] {
            if requested > cap {
                return Err(SanitizeError::LimitExceedsHardCap { kind, cap });
            }
        }
        Ok(Self {
            limits,
            remote_images,
        })
    }

    #[must_use]
    pub const fn limits(self) -> PolicyLimits {
        self.limits
    }

    #[must_use]
    pub const fn remote_images(self) -> RemoteImagePolicy {
        self.remote_images
    }
}

impl Default for ContentPolicy {
    fn default() -> Self {
        Self {
            limits: PolicyLimits::default(),
            remote_images: RemoteImagePolicy::Deny,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SanitizeError {
    LimitExceedsHardCap {
        kind: &'static str,
        cap: usize,
    },
    InputTooLarge {
        kind: &'static str,
        limit: usize,
    },
    OutputTooLarge {
        limit: usize,
    },
    NestingTooDeep {
        limit: usize,
    },
    TooComplex,
    Serialization,
    UnsafeUrl,
    /// The caller's cancellation callback returned `true`.
    Cancelled,
}

impl fmt::Display for SanitizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceedsHardCap { kind, cap } => {
                write!(f, "{kind} limit exceeds the absolute {cap}-byte cap")
            }
            Self::InputTooLarge { kind, limit } => {
                write!(f, "{kind} input exceeds the {limit}-byte policy limit")
            }
            Self::OutputTooLarge { limit } => {
                write!(f, "sanitized output exceeds the {limit}-byte policy limit")
            }
            Self::NestingTooDeep { limit } => {
                write!(f, "markup nests deeper than the {limit}-level policy limit")
            }
            Self::TooComplex => f.write_str("markup exceeds the tree-construction work budget"),
            Self::Serialization => f.write_str("sanitized output could not be serialized"),
            Self::UnsafeUrl => f.write_str("URL rejected by content policy"),
            Self::Cancelled => f.write_str("sanitization was cancelled"),
        }
    }
}

impl Error for SanitizeError {}

/// HTML proven safe by this crate. Its private representation prevents callers
/// from relabelling parser output or arbitrary strings as sanitized content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SanitizedFragment(String);

impl SanitizedFragment {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Inline SVG emitted by the parser-backed SVG policy. It is deliberately
/// distinct from HTML and cannot be constructed by WebView or rendering
/// callers.
///
/// This is **inline-only** markup: HTML-serialized foreign content meant to
/// be embedded in an HTML document. It is not a standalone
/// `image/svg+xml` document. It has no `xmlns`, it can contain several roots
/// or HTML siblings, and it uses HTML serialization rules. Never serve it as
/// an SVG file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SanitizedSvg(String);

impl SanitizedSvg {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Embed the sanitized SVG as an HTML fragment. The same policy and the
    /// same `<div>`-context fragment parse produced it, so it is a sanitized
    /// fragment as-is.
    #[must_use]
    pub fn into_fragment(self) -> SanitizedFragment {
        SanitizedFragment(self.0)
    }
}

/// Parser-backed HTML boundary. Output is produced only by the sanitizer, so
/// callers cannot relabel arbitrary strings as `SanitizedFragment`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtmlPolicy {
    policy: ContentPolicy,
}

impl HtmlPolicy {
    #[must_use]
    pub const fn new(policy: ContentPolicy) -> Self {
        Self { policy }
    }

    /// Parse `html` as a `<div>` fragment and keep only the allowlisted
    /// elements, attributes, URLs and CSS declarations. The input budget is
    /// checked before parsing and the output budget while serializing; either
    /// failure returns an error and no partial output.
    pub fn sanitize_fragment(&self, html: &str) -> Result<SanitizedFragment, SanitizeError> {
        enforce_input_limit(html, "HTML", self.policy.limits.max_html_input_bytes)?;
        html::sanitize(html, self.policy, html::Run::DEFAULT).map(SanitizedFragment)
    }

    /// [`Self::sanitize_fragment`] that polls `cancel` between parser pieces
    /// and chunks (at most about 16 KiB of input apart) and returns
    /// [`SanitizeError::Cancelled`], with no partial output, once it returns
    /// `true`. `cancel` may be called from several threads.
    pub fn sanitize_fragment_cancellable(
        &self,
        html: &str,
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<SanitizedFragment, SanitizeError> {
        enforce_input_limit(html, "HTML", self.policy.limits.max_html_input_bytes)?;
        let run = html::Run {
            cancel: Some(cancel),
            ..html::Run::DEFAULT
        };
        html::sanitize(html, self.policy, run).map(SanitizedFragment)
    }

    /// Encode untrusted plain text as inert HTML. This is not an HTML sanitizer.
    pub fn encode_text(&self, text: &str) -> Result<SanitizedFragment, SanitizeError> {
        enforce_input_limit(text, "HTML", self.policy.limits.max_html_input_bytes)?;
        let mut output = String::with_capacity(text.len());
        for character in text.chars() {
            match character {
                '&' => output.push_str("&amp;"),
                '<' => output.push_str("&lt;"),
                '>' => output.push_str("&gt;"),
                '"' => output.push_str("&quot;"),
                '\'' => output.push_str("&#39;"),
                _ => output.push(character),
            }
            if output.len() > self.policy.limits.max_html_output_bytes {
                return Err(SanitizeError::OutputTooLarge {
                    limit: self.policy.limits.max_html_output_bytes,
                });
            }
        }
        Ok(SanitizedFragment(output))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SvgPolicy {
    policy: ContentPolicy,
}

impl SvgPolicy {
    #[must_use]
    pub const fn new(policy: ContentPolicy) -> Self {
        Self { policy }
    }

    /// Sanitize SVG markup with the same parser and allowlists as the .NET
    /// `SanitizeSvg` (which also runs the HTML sanitizer). Scripts, event
    /// handlers, foreign-namespace escapes and non-fragment `<use>`
    /// references are removed. Output is bounded by the HTML output budget.
    pub fn sanitize(&self, svg: &str) -> Result<SanitizedSvg, SanitizeError> {
        enforce_input_limit(svg, "SVG", self.policy.limits.max_svg_input_bytes)?;
        html::sanitize(svg, self.policy, html::Run::DEFAULT).map(SanitizedSvg)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CssPolicy {
    policy: ContentPolicy,
}

impl CssPolicy {
    #[must_use]
    pub const fn new(policy: ContentPolicy) -> Self {
        Self { policy }
    }

    /// Sanitize the body of an inline `style` attribute: only Ganss
    /// `AllowedCssProperties` survive, and any declaration carrying `url()`,
    /// `expression()` or another active construct is dropped. Returns the
    /// retained declarations as `name: value` joined by `"; "` (possibly empty).
    pub fn sanitize_inline_style(&self, css: &str) -> Result<String, SanitizeError> {
        enforce_input_limit(css, "CSS", self.policy.limits.max_css_input_bytes)?;
        Ok(css::sanitize_declarations(css))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UrlContext {
    Link,
    Image,
    Css,
    SvgReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafeUrl(String);

impl SafeUrl {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UrlPolicy {
    policy: ContentPolicy,
}

impl UrlPolicy {
    #[must_use]
    pub const fn new(policy: ContentPolicy) -> Self {
        Self { policy }
    }

    /// Validate an already entity-decoded attribute value. Links permit a
    /// deliberately conservative subset of HTTP(S), mailto, and tel forms,
    /// independently of the remote-image setting. Images require explicit
    /// HTTPS permission. Ports, IP literals, trailing-dot hosts, mail headers,
    /// telephone extensions, protocol-relative URLs, and all other active
    /// schemes fail closed pending a parser-backed compatibility decision.
    pub fn validate(&self, value: &str, context: UrlContext) -> Result<SafeUrl, SanitizeError> {
        if value.len() > self.policy.limits.max_url_bytes
            || value.is_empty()
            || value != value.trim()
            || value.chars().any(|c| c.is_control() || c == '\\')
            || value.starts_with("//")
        {
            return Err(SanitizeError::UnsafeUrl);
        }
        if value.starts_with('#') {
            return Ok(SafeUrl(value.to_owned()));
        }
        let lower = value.to_ascii_lowercase();
        if let Some(authority_tail) = lower.strip_prefix("https://") {
            if !valid_web_authority(authority_tail) {
                return Err(SanitizeError::UnsafeUrl);
            }
            if context == UrlContext::Image
                && self.policy.remote_images != RemoteImagePolicy::AllowHttps
            {
                return Err(SanitizeError::UnsafeUrl);
            }
            if matches!(context, UrlContext::Link | UrlContext::Image) {
                return Ok(SafeUrl(value.to_owned()));
            }
            return Err(SanitizeError::UnsafeUrl);
        }
        if let Some(authority_tail) = lower.strip_prefix("http://") {
            if context == UrlContext::Link && valid_web_authority(authority_tail) {
                return Ok(SafeUrl(value.to_owned()));
            }
            return Err(SanitizeError::UnsafeUrl);
        }
        if context == UrlContext::Link
            && ((lower.starts_with("mailto:") && valid_mailto(&value[7..]))
                || (lower.starts_with("tel:") && valid_tel(&value[4..])))
        {
            return Ok(SafeUrl(value.to_owned()));
        }
        let first_separator = value.find(['/', '?', '#']).unwrap_or(value.len());
        if value[..first_separator].contains(':')
            || value.starts_with('/')
            || matches!(context, UrlContext::Css | UrlContext::SvgReference)
        {
            return Err(SanitizeError::UnsafeUrl);
        }
        Ok(SafeUrl(value.to_owned()))
    }
}

fn valid_web_authority(after_scheme: &str) -> bool {
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    !authority.is_empty()
        && !authority.contains('@')
        && authority.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn valid_mailto(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && !domain.contains(['/', '?', '#'])
        && local.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'%' | b'+' | b'-')
        })
        && valid_web_authority(domain)
}

fn valid_tel(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_digit() || matches!(character, '+' | '-' | '(' | ')' | '.' | ' ')
        })
}

/// Test hooks for the chunking differential tests. Not part of the API.
#[doc(hidden)]
pub mod testing {
    use crate::{ContentPolicy, SanitizeError, html};

    /// Sanitize with an explicit chunk target (`None`: the whole input as one
    /// chunk) and worker bound, returning the plain string. The input limit
    /// is not checked.
    pub fn sanitize_with_chunking(
        policy: ContentPolicy,
        input: &str,
        chunk_target: Option<usize>,
        max_workers: usize,
    ) -> Result<String, SanitizeError> {
        let target = chunk_target.unwrap_or(usize::MAX);
        let run = html::Run {
            cancel: None,
            chunk_target: target,
            segment_target: target,
            max_workers,
        };
        html::sanitize(input, policy, run)
    }

    /// The chunks the default settings cut `input` into (byte ranges).
    pub fn chunk_ranges(input: &str) -> Result<Vec<std::ops::Range<usize>>, SanitizeError> {
        html::chunk_ranges(input, html::Run::DEFAULT)
    }
}

fn enforce_input_limit(input: &str, kind: &'static str, limit: usize) -> Result<(), SanitizeError> {
    if input.len() > limit {
        Err(SanitizeError::InputTooLarge { kind, limit })
    } else {
        Ok(())
    }
}
