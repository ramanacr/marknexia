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
}

impl MarkdownOptions {
    /// .NET `MarkdownRenderer.MaxDiagramCount`.
    pub const DEFAULT_MAX_DIAGRAM_COUNT: usize = 64;
    /// .NET `MarkdownRenderer.MaxDiagramSourceBytes`.
    pub const DEFAULT_MAX_DIAGRAM_SOURCE_BYTES: usize = 1024 * 1024;
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            max_diagram_count: Self::DEFAULT_MAX_DIAGRAM_COUNT,
            max_diagram_source_bytes: Self::DEFAULT_MAX_DIAGRAM_SOURCE_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Heading {
    pub text: String,
    pub level: u8,
    pub slug_id: String,
    pub line_number: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Anchor {
    pub id: String,
    pub name: String,
    pub is_heading_anchor: bool,
    pub line_number: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Diagram {
    pub id: String,
    pub diagram_type: String,
    pub source_code: String,
    pub line_number: usize,
}

/// Parsed Markdown. `rendered_body_html` is raw, unsanitized, content-unsafe
/// HTML; it must pass the sanitizer and output limits before reaching WebView.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedDocument {
    pub rendered_body_html: String,
    pub headings: Vec<Heading>,
    pub custom_anchors: Vec<Anchor>,
    pub links: Vec<String>,
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
