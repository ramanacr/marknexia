use marknexia_core::contracts::Diagnostic;
use serde::Serialize;

#[derive(Clone, Debug, Default)]
pub struct MarkdownOptions;

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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedDocument {
    pub rendered_body_html: String,
    pub headings: Vec<Heading>,
    pub custom_anchors: Vec<Anchor>,
    pub links: Vec<String>,
    pub images: Vec<String>,
    pub diagrams: Vec<Diagram>,
}

pub trait MarkdownEngine {
    fn parse(
        &self,
        source: &str,
        options: &MarkdownOptions,
    ) -> Result<ParsedDocument, Vec<Diagnostic>>;
}
