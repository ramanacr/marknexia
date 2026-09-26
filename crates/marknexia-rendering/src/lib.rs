#![forbid(unsafe_code)]

//! Markdown → sanitized document, reproducing the .NET `MarkdownRenderer`
//! pipeline (`src/Marknexia.Rendering`):
//!
//! 1. parse with a [`MarkdownEngine`] (raw, unsanitized body HTML);
//! 2. wrap code blocks in the copy-button container and highlight C#;
//! 3. replace Mermaid blocks with the diagram shell or a limit fallback;
//! 4. render math spans with the local TeX subset;
//! 5. sanitize with [`HtmlPolicy::sanitize_fragment`] and, with remote assets
//!    off, blank remote image sources exactly where .NET does;
//! 6. enforce the full-page size limit of the .NET template.
//!
//! Unsanitized markup never leaves this crate: [`RenderedDocument`] holds the
//! body only as a [`SanitizedFragment`] and all metadata as [`PlainText`],
//! which reaches HTML only through [`PlainText::encode_html`]. The shell builds
//! a WebView `HostDocument` from [`RenderedDocument::body`];
//! [`RenderedDocument::page_html`] reproduces the complete .NET page (trusted
//! template + sanitized body) for parity and for the .NET-compatible host.
//!
//! Differences from .NET are recorded in
//! `compat/decisions/rendering-differences.md`.

mod highlight;
mod math;
mod passes;
mod template;
mod text;

use std::{error::Error, fmt};

use marknexia_core::contracts::{AppTheme, Diagnostic};
use marknexia_markdown::{MarkdownEngine, MarkdownOptions, ParsedDocument};
use marknexia_security::{
    ContentPolicy, HtmlPolicy, PolicyLimits, RemoteImagePolicy, SanitizeError, SanitizedFragment,
};

pub use math::MAX_MATH_DEPTH;
pub use template::{InvalidPageIdentity, PageIdentity};

/// Resource limits. Defaults are the .NET constants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderLimits {
    /// `FileService.LargeFileThresholdBytes` (50 MiB): larger sources are
    /// rejected before they are read.
    pub max_source_bytes: u64,
    /// `MarkdownRenderer.MaxRenderedHtmlBytes` (128 MiB): limit on the
    /// complete templated page.
    pub max_rendered_html_bytes: usize,
    /// `MarkdownRenderer.MaxDiagramCount`.
    pub max_diagram_count: usize,
    /// `MarkdownRenderer.MaxDiagramSourceBytes`.
    pub max_diagram_source_bytes: usize,
}

impl RenderLimits {
    pub const DEFAULT_MAX_SOURCE_BYTES: u64 = 50 * 1024 * 1024;
    pub const DEFAULT_MAX_RENDERED_HTML_BYTES: usize = 128 * 1024 * 1024;
    pub const DEFAULT_MAX_DIAGRAM_COUNT: usize = 64;
    pub const DEFAULT_MAX_DIAGRAM_SOURCE_BYTES: usize = 1024 * 1024;
}

impl Default for RenderLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: Self::DEFAULT_MAX_SOURCE_BYTES,
            max_rendered_html_bytes: Self::DEFAULT_MAX_RENDERED_HTML_BYTES,
            max_diagram_count: Self::DEFAULT_MAX_DIAGRAM_COUNT,
            max_diagram_source_bytes: Self::DEFAULT_MAX_DIAGRAM_SOURCE_BYTES,
        }
    }
}

/// Per-document settings (.NET `RenderContext` plus the template secrets).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderContext {
    pub theme: AppTheme,
    pub allow_remote_assets: bool,
    pub enable_diagrams: bool,
    pub enable_math: bool,
    /// The document's directory relative to its asset root, `/`- or
    /// `\`-separated; empty or `.` for the root. It becomes the `<base href>`
    /// path, one percent-encoded segment each.
    pub document_directory: String,
    pub identity: PageIdentity,
}

impl RenderContext {
    /// .NET defaults: diagrams and math on, remote assets off.
    #[must_use]
    pub fn new(theme: AppTheme, identity: PageIdentity) -> Self {
        Self {
            theme,
            allow_remote_assets: false,
            enable_diagrams: true,
            enable_math: true,
            document_directory: String::new(),
            identity,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderError {
    /// A limit in [`RenderLimits`] is zero (.NET `ArgumentOutOfRangeException`).
    InvalidLimits,
    /// The document directory escapes its asset root.
    InvalidDocumentDirectory,
    /// .NET `DocumentTooLargeException` from `FileService` (the fixture's
    /// `rejected-before-read`).
    SourceTooLarge { size_bytes: u64, maximum_bytes: u64 },
    /// .NET `DocumentTooLargeException` from the renderer (`rejected`). The
    /// size is unknown when the Markdown layer stopped at the limit.
    RenderedTooLarge {
        size_bytes: Option<u64>,
        maximum_bytes: u64,
    },
    /// The sanitizer refused the body (its budgets are lower than .NET's;
    /// REND-1). No partial output exists.
    Sanitizer(SanitizeError),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits => f.write_str("render limits must be positive"),
            Self::InvalidDocumentDirectory => {
                f.write_str("the document is outside its authorized asset root")
            }
            Self::SourceTooLarge {
                size_bytes,
                maximum_bytes,
            } => write!(
                f,
                "document is {size_bytes} bytes; the limit is {maximum_bytes} bytes"
            ),
            Self::RenderedTooLarge {
                size_bytes: Some(size),
                maximum_bytes,
            } => write!(
                f,
                "rendered document is {size} bytes; the limit is {maximum_bytes} bytes"
            ),
            Self::RenderedTooLarge {
                size_bytes: None,
                maximum_bytes,
            } => write!(
                f,
                "rendered document exceeds the {maximum_bytes}-byte limit"
            ),
            Self::Sanitizer(error) => write!(f, "sanitizer rejected the document: {error}"),
        }
    }
}

impl Error for RenderError {}

/// Untrusted text (heading text, slugs, anchor names, image sources, diagram
/// source). It is never markup: use [`PlainText::as_str`] for text sinks and
/// [`PlainText::encode_html`] to place it in HTML.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PlainText(String);

impl PlainText {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Entity-encode for an HTML text or attribute context.
    pub fn encode_html(&self, policy: &HtmlPolicy) -> Result<SanitizedFragment, SanitizeError> {
        policy.encode_text(&self.0)
    }
}

impl From<String> for PlainText {
    fn from(text: String) -> Self {
        Self(text)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedHeading {
    pub text: PlainText,
    pub level: u8,
    pub slug_id: PlainText,
    pub line_number: usize,
}

/// One entry of the .NET anchor index (`AnchorTarget`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchorTarget {
    pub id: PlainText,
    pub text: PlainText,
    pub is_heading_anchor: bool,
    pub line_number: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedDiagram {
    pub id: PlainText,
    pub diagram_type: PlainText,
    pub source_code: PlainText,
    pub line_number: usize,
}

/// A rendered document: sanitized body plus plain-text metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedDocument {
    body: SanitizedFragment,
    head: template::PageHead,
    page_bytes: usize,
    has_mermaid: bool,
    headings: Vec<RenderedHeading>,
    anchors: Vec<AnchorTarget>,
    image_references: Vec<PlainText>,
    diagrams: Vec<RenderedDiagram>,
    diagnostics: Vec<Diagnostic>,
}

impl RenderedDocument {
    /// The sanitized Markdown body (the content of `<div class="markdown-body">`).
    #[must_use]
    pub const fn body(&self) -> &SanitizedFragment {
        &self.body
    }

    #[must_use]
    pub fn into_body(self) -> SanitizedFragment {
        self.body
    }

    /// The complete .NET `TemplateEngine` page: trusted template text, bundled
    /// CSS and bridge script, and the sanitized body.
    #[must_use]
    pub fn page_html(&self) -> String {
        let mut page = String::with_capacity(self.page_bytes);
        self.head.write(self.body.as_str(), &mut page);
        page
    }

    /// UTF-8 length of [`Self::page_html`], the value the output limit checks.
    #[must_use]
    pub const fn page_bytes(&self) -> usize {
        self.page_bytes
    }

    /// A Mermaid diagram shell was emitted, so the page loads the Mermaid
    /// runtime (.NET `hasMermaid && EnableDiagrams`).
    #[must_use]
    pub const fn has_mermaid(&self) -> bool {
        self.has_mermaid
    }

    #[must_use]
    pub fn headings(&self) -> &[RenderedHeading] {
        &self.headings
    }

    /// The .NET anchor index in insertion order: headings first, then custom
    /// anchors, first occurrence winning under ordinal case-insensitive ids.
    #[must_use]
    pub fn anchors(&self) -> &[AnchorTarget] {
        &self.anchors
    }

    /// Raw image sources as written (.NET `AssetReferences`).
    #[must_use]
    pub fn image_references(&self) -> &[PlainText] {
        &self.image_references
    }

    #[must_use]
    pub fn diagrams(&self) -> &[RenderedDiagram] {
        &self.diagrams
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// A Markdown document renderer.
pub trait DocumentRenderer {
    fn render(
        &self,
        source: &str,
        context: &RenderContext,
    ) -> Result<RenderedDocument, RenderError>;
}

/// The .NET `MarkdownRenderer` over a Marknexia Markdown engine.
#[derive(Clone)]
pub struct Renderer<E> {
    engine: E,
    limits: RenderLimits,
}

impl<E> fmt::Debug for Renderer<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Renderer")
            .field("engine", &std::any::type_name::<E>())
            .field("limits", &self.limits)
            .finish()
    }
}

impl Renderer<marknexia_markdown::PulldownAdapter> {
    /// The recommended Markdown candidate with .NET default limits.
    #[must_use]
    pub fn new() -> Self {
        Self {
            engine: marknexia_markdown::PulldownAdapter,
            limits: RenderLimits::default(),
        }
    }
}

impl Default for Renderer<marknexia_markdown::PulldownAdapter> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: MarkdownEngine> Renderer<E> {
    pub fn with_engine(engine: E, limits: RenderLimits) -> Result<Self, RenderError> {
        if limits.max_source_bytes == 0
            || limits.max_rendered_html_bytes == 0
            || limits.max_diagram_source_bytes == 0
        {
            return Err(RenderError::InvalidLimits);
        }
        Ok(Self { engine, limits })
    }

    #[must_use]
    pub const fn limits(&self) -> RenderLimits {
        self.limits
    }

    /// The pre-read size check (.NET `FileService.ReadTextAsync`): call with
    /// the file length before reading it.
    pub fn check_source_size(&self, size_bytes: u64) -> Result<(), RenderError> {
        if size_bytes > self.limits.max_source_bytes {
            Err(RenderError::SourceTooLarge {
                size_bytes,
                maximum_bytes: self.limits.max_source_bytes,
            })
        } else {
            Ok(())
        }
    }

    fn render_document(
        &self,
        source: &str,
        context: &RenderContext,
    ) -> Result<RenderedDocument, RenderError> {
        self.check_source_size(source.len() as u64)?;
        let segments = directory_segments(&context.document_directory)?;
        let maximum_bytes = self.limits.max_rendered_html_bytes;
        let too_large = |size_bytes: Option<usize>| RenderError::RenderedTooLarge {
            size_bytes: size_bytes.map(|size| size as u64),
            maximum_bytes: maximum_bytes as u64,
        };
        let options = MarkdownOptions {
            max_diagram_count: self.limits.max_diagram_count,
            max_diagram_source_bytes: self.limits.max_diagram_source_bytes,
            // The body can never exceed the page, so the parser may stop here.
            max_rendered_body_bytes: maximum_bytes,
        };
        let parsed = self
            .engine
            .parse(source, &options)
            .map_err(|_| too_large(None))?;

        // Every pass runs within the sanitizer's input budget, which binds
        // before the page limit; passes check estimates before building each
        // block, so intermediate strings stay near the budget.
        let sanitizer_limits = PolicyLimits::default();
        let budget = sanitizer_limits.max_html_input_bytes;
        let over_budget = |_| {
            RenderError::Sanitizer(SanitizeError::InputTooLarge {
                kind: "HTML",
                limit: budget,
            })
        };
        if parsed.rendered_body_html.len() > budget {
            return Err(over_budget(passes::Overflow));
        }
        let body = passes::highlight_code_blocks(&parsed.rendered_body_html, budget)
            .map_err(over_budget)?;
        let (body, has_mermaid) = passes::transform_diagrams(
            &body,
            passes::DiagramSettings {
                enabled: context.enable_diagrams,
                max_count: self.limits.max_diagram_count,
                max_source_bytes: self.limits.max_diagram_source_bytes,
            },
            budget,
        )
        .map_err(over_budget)?;
        let body = passes::render_math(&body, context.enable_math, budget).map_err(over_budget)?;
        let body = sanitize_body(&body, context.allow_remote_assets, sanitizer_limits)?;

        let head = template::PageHead::new(
            context.theme,
            context.allow_remote_assets,
            &segments,
            &context.identity,
            has_mermaid && context.enable_diagrams,
        );
        let mut counter = template::Counter(0);
        head.write(body.as_str(), &mut counter);
        if counter.0 > maximum_bytes {
            return Err(too_large(Some(counter.0)));
        }
        Ok(assemble(parsed, body, head, counter.0, has_mermaid))
    }
}

impl<E: MarkdownEngine> DocumentRenderer for Renderer<E> {
    fn render(
        &self,
        source: &str,
        context: &RenderContext,
    ) -> Result<RenderedDocument, RenderError> {
        self.render_document(source, context)
    }
}

/// Steps 4 and 4b. .NET sanitizes (keeping remote images) and then blanks
/// remote `src`/`srcset` on the sanitized markup. Rust does the same, then
/// sanitizes the blanked markup again under the deny policy so the result is
/// a `SanitizedFragment` produced under the document's real policy. The
/// remote-image policy only affects `src` attributes, so without a `src` in
/// the input one deny-policy pass is equivalent.
fn sanitize_body(
    body: &str,
    allow_remote_assets: bool,
    limits: PolicyLimits,
) -> Result<SanitizedFragment, RenderError> {
    let policy = |remote| {
        ContentPolicy::try_new(limits, remote)
            .map(HtmlPolicy::new)
            .map_err(RenderError::Sanitizer)
    };
    let allow = policy(RemoteImagePolicy::AllowHttps)?;
    if allow_remote_assets {
        return allow
            .sanitize_fragment(body)
            .map_err(RenderError::Sanitizer);
    }
    let deny = policy(RemoteImagePolicy::Deny)?;
    if text::find_ci(body, 0, "src").is_none() {
        return deny.sanitize_fragment(body).map_err(RenderError::Sanitizer);
    }
    let permissive = allow
        .sanitize_fragment(body)
        .map_err(RenderError::Sanitizer)?;
    let blanked = passes::block_remote_images(permissive.as_str());
    deny.sanitize_fragment(&blanked)
        .map_err(RenderError::Sanitizer)
}

/// .NET `DocumentAssetContext.Create`: the document directory relative to
/// the asset root, which must not escape it.
fn directory_segments(directory: &str) -> Result<Vec<&str>, RenderError> {
    if directory.is_empty() || directory == "." {
        return Ok(Vec::new());
    }
    let segments: Vec<&str> = directory.split(['/', '\\']).collect();
    if segments.iter().any(|segment| {
        segment.is_empty() || *segment == "." || *segment == ".." || segment.contains(':')
    }) {
        return Err(RenderError::InvalidDocumentDirectory);
    }
    Ok(segments)
}

/// .NET `StringComparer.OrdinalIgnoreCase` key: simple per-character upper
/// case.
fn ordinal_ignore_case_key(text: &str) -> String {
    text.chars()
        .map(|character| {
            let mut upper = character.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(single), None) => single,
                _ => character,
            }
        })
        .collect()
}

fn assemble(
    parsed: ParsedDocument,
    body: SanitizedFragment,
    head: template::PageHead,
    page_bytes: usize,
    has_mermaid: bool,
) -> RenderedDocument {
    let mut seen = std::collections::HashSet::new();
    let mut anchors = Vec::new();
    for heading in &parsed.headings {
        if seen.insert(ordinal_ignore_case_key(&heading.slug_id)) {
            anchors.push(AnchorTarget {
                id: PlainText(heading.slug_id.clone()),
                text: PlainText(heading.text.clone()),
                is_heading_anchor: true,
                line_number: heading.line_number,
            });
        }
    }
    for anchor in &parsed.custom_anchors {
        if seen.insert(ordinal_ignore_case_key(&anchor.id)) {
            anchors.push(AnchorTarget {
                id: PlainText(anchor.id.clone()),
                text: PlainText(anchor.name.clone()),
                is_heading_anchor: anchor.is_heading_anchor,
                line_number: anchor.line_number,
            });
        }
    }
    RenderedDocument {
        body,
        head,
        page_bytes,
        has_mermaid,
        headings: parsed
            .headings
            .into_iter()
            .map(|heading| RenderedHeading {
                text: PlainText(heading.text),
                level: heading.level,
                slug_id: PlainText(heading.slug_id),
                line_number: heading.line_number,
            })
            .collect(),
        anchors,
        image_references: parsed.images.into_iter().map(PlainText).collect(),
        diagrams: parsed
            .diagrams
            .into_iter()
            .map(|diagram| RenderedDiagram {
                id: PlainText(diagram.id),
                diagram_type: PlainText(diagram.diagram_type),
                source_code: PlainText(diagram.source_code),
                line_number: diagram.line_number,
            })
            .collect(),
        diagnostics: parsed.diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_validation() {
        assert_eq!(directory_segments("").unwrap(), Vec::<&str>::new());
        assert_eq!(directory_segments(".").unwrap(), Vec::<&str>::new());
        assert_eq!(directory_segments("a\\b/c").unwrap(), ["a", "b", "c"]);
        for bad in ["..", "a/../b", "/a", "a//b", "a/", "C:/x", "./a"] {
            assert_eq!(
                directory_segments(bad),
                Err(RenderError::InvalidDocumentDirectory),
                "{bad}"
            );
        }
    }

    #[test]
    fn anchor_keys_ignore_case() {
        assert_eq!(ordinal_ignore_case_key("Ab-ß"), "AB-ß");
        assert_eq!(ordinal_ignore_case_key("é"), "É");
    }
}
