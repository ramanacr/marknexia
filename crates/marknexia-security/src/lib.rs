#![forbid(unsafe_code)]

//! Security-owned content boundaries.
//!
//! [`SanitizedFragment`] cannot be constructed outside this crate. The current
//! feasibility workspace has no locked parser-backed sanitizer, so
//! [`HtmlPolicy`] deliberately rejects markup rather than approximating a
//! sanitizer with string replacement. Plain text can still cross the boundary
//! after deterministic HTML escaping.

use std::{error::Error, fmt};

use serde::Serialize;

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
            max_html_input_bytes: 4 * 1024 * 1024,
            max_html_output_bytes: 8 * 1024 * 1024,
            max_svg_input_bytes: 512 * 1024,
            max_css_input_bytes: 64 * 1024,
            max_url_bytes: 8 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteImagePolicy {
    Deny,
    AllowHttps,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentPolicy {
    limits: PolicyLimits,
    remote_images: RemoteImagePolicy,
}

impl ContentPolicy {
    #[must_use]
    pub const fn new(limits: PolicyLimits, remote_images: RemoteImagePolicy) -> Self {
        Self {
            limits,
            remote_images,
        }
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
        Self::new(PolicyLimits::default(), RemoteImagePolicy::Deny)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SanitizeError {
    InputTooLarge { kind: &'static str, limit: usize },
    OutputTooLarge { limit: usize },
    ParserUnavailable,
    CssParserUnavailable,
    SvgParserUnavailable,
    UnsafeUrl,
}

impl fmt::Display for SanitizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge { kind, limit } => {
                write!(f, "{kind} input exceeds the {limit}-byte policy limit")
            }
            Self::OutputTooLarge { limit } => {
                write!(f, "sanitized output exceeds the {limit}-byte policy limit")
            }
            Self::ParserUnavailable => f.write_str("parser-backed HTML sanitizer unavailable"),
            Self::CssParserUnavailable => f.write_str("parser-backed CSS sanitizer unavailable"),
            Self::SvgParserUnavailable => f.write_str("parser-backed SVG sanitizer unavailable"),
            Self::UnsafeUrl => f.write_str("URL rejected by content policy"),
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

/// Fail-closed HTML boundary. A parser-backed implementation can replace the
/// unavailable branch without exposing construction of `SanitizedFragment`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtmlPolicy {
    policy: ContentPolicy,
}

impl HtmlPolicy {
    #[must_use]
    pub const fn new(policy: ContentPolicy) -> Self {
        Self { policy }
    }

    /// No parser is locked in this feasibility tree, so non-empty markup is
    /// rejected explicitly and without partial output.
    pub fn sanitize_fragment(&self, html: &str) -> Result<SanitizedFragment, SanitizeError> {
        enforce_input_limit(html, "HTML", self.policy.limits.max_html_input_bytes)?;
        if html.is_empty() {
            return Ok(SanitizedFragment(String::new()));
        }
        Err(SanitizeError::ParserUnavailable)
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

    pub fn sanitize(&self, svg: &str) -> Result<SanitizedFragment, SanitizeError> {
        enforce_input_limit(svg, "SVG", self.policy.limits.max_svg_input_bytes)?;
        Err(SanitizeError::SvgParserUnavailable)
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

    /// Inline style input is rejected until a real CSS parser is locked.
    pub fn sanitize_inline_style(&self, css: &str) -> Result<String, SanitizeError> {
        enforce_input_limit(css, "CSS", self.policy.limits.max_css_input_bytes)?;
        Err(SanitizeError::CssParserUnavailable)
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

    /// Validate an already entity-decoded attribute value. Only fragments,
    /// conservative relative references, and explicitly permitted HTTPS image
    /// URLs are accepted. Protocol-relative URLs and active schemes fail.
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
        if lower.starts_with("https://") {
            if context != UrlContext::Image
                || self.policy.remote_images != RemoteImagePolicy::AllowHttps
                || !valid_https_authority(&value[8..])
            {
                return Err(SanitizeError::UnsafeUrl);
            }
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

fn valid_https_authority(after_scheme: &str) -> bool {
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

fn enforce_input_limit(input: &str, kind: &'static str, limit: usize) -> Result<(), SanitizeError> {
    if input.len() > limit {
        Err(SanitizeError::InputTooLarge { kind, limit })
    } else {
        Ok(())
    }
}
