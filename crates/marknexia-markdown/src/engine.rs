use marknexia_core::contracts::Diagnostic;
use serde::Serialize;

/// Parser options. Defaults mirror the .NET `MarkdownRenderer` limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarkdownOptions {
    /// Diagrams beyond this count produce a source-positioned diagnostic.
    pub max_diagram_count: usize,
    /// Diagrams whose rendered source exceeds this many UTF-8 bytes produce a
    /// source-positioned diagnostic.
    pub max_diagram_source_bytes: usize,
    /// Parsing fails with an `Error` diagnostic once the rendered body would
    /// exceed this many UTF-8 bytes. Rendering stops at the limit, so output
    /// memory is bounded. The .NET renderer applies the same limit to the full
    /// document (`DocumentTooLargeException`); the rendering layer must still
    /// enforce its own full-document limit.
    pub max_rendered_body_bytes: usize,
}

impl MarkdownOptions {
    /// .NET `MarkdownRenderer.MaxDiagramCount`.
    pub const DEFAULT_MAX_DIAGRAM_COUNT: usize = 64;
    /// .NET `MarkdownRenderer.MaxDiagramSourceBytes`.
    pub const DEFAULT_MAX_DIAGRAM_SOURCE_BYTES: usize = 1024 * 1024;
    /// .NET `MarkdownRenderer.MaxRenderedHtmlBytes` (128 MiB).
    pub const DEFAULT_MAX_RENDERED_BODY_BYTES: usize = 128 * 1024 * 1024;
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            max_diagram_count: Self::DEFAULT_MAX_DIAGRAM_COUNT,
            max_diagram_source_bytes: Self::DEFAULT_MAX_DIAGRAM_SOURCE_BYTES,
            max_rendered_body_bytes: Self::DEFAULT_MAX_RENDERED_BODY_BYTES,
        }
    }
}

/// A heading. `text` is raw, unescaped source text (it may contain `<`, `&`,
/// quotes, or script-like content): consumers must escape it for their output
/// context. `slug_id` is derived from it and must be escaped the same way.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Heading {
    pub text: String,
    pub level: u8,
    pub slug_id: String,
    pub line_number: usize,
}

/// A custom `<a id|name>` anchor. `id` and `name` are the raw attribute text
/// from untrusted HTML, not decoded or validated: consumers must escape them
/// and must not treat them as safe identifiers or URLs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Anchor {
    pub id: String,
    pub name: String,
    pub is_heading_anchor: bool,
    pub line_number: usize,
}

/// A Mermaid block. `source_code` is the raw, unescaped block text:
/// consumers must escape it (or pass it only to a sandboxed diagram renderer).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Diagram {
    pub id: String,
    pub diagram_type: String,
    pub source_code: String,
    pub line_number: usize,
}

/// Parsed Markdown. `rendered_body_html` is raw, unsanitized, content-unsafe
/// HTML; it must pass an HTML5-parser-based sanitizer before reaching WebView.
/// Every string field is raw, untrusted text that consumers must escape for
/// their output context.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedDocument {
    pub rendered_body_html: String,
    pub headings: Vec<Heading>,
    pub custom_anchors: Vec<Anchor>,
    /// Raw link destinations (not URL-validated or escaped).
    pub links: Vec<String>,
    /// Raw image sources (not URL-validated or escaped).
    pub images: Vec<String>,
    pub diagrams: Vec<Diagram>,
    /// Source-positioned warnings. Not part of the frozen `marknexia-parity-v1`
    /// schema (the .NET adapter always returns none); see
    /// `compat/decisions/markdown-source-diagnostics.md`.
    #[serde(skip)]
    pub diagnostics: Vec<Diagnostic>,
}

pub trait MarkdownEngine {
    fn parse(
        &self,
        source: &str,
        options: &MarkdownOptions,
    ) -> Result<ParsedDocument, Vec<Diagnostic>>;
}
