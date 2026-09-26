//! Marknexia-owned compatibility layer shared by every parser candidate.
//!
//! Adapters only translate third-party syntax trees into [`model::Document`].
//! Everything the .NET Markdig 0.40 pipeline adds on top of CommonMark lives
//! here: Markdig-shaped HTML, heading IDs, GitHub alerts, `==mark==`, grid
//! tables, footnote markup, task-list markup, code language labels, Mermaid
//! extraction and limits, link/image/anchor extraction, and diagnostics.
//!
//! Output is raw, unsanitized, content-unsafe HTML.

mod alerts;
mod anchors;
mod grid_table;
mod mark;
mod mermaid;
pub(crate) mod model;
mod render;

use crate::{MarkdownOptions, ParsedDocument};
use model::{Document, normalize_line_endings};

/// A third-party parser translated into the Marknexia document model.
pub(crate) trait Frontend {
    /// Whether the parser itself produces [`model::Inline::Mark`] for `==text==`.
    const NATIVE_MARK: bool;

    /// Parses `source`, whose line endings are already `\n`.
    fn parse_document(&self, source: &str) -> Document;
}

pub(crate) fn parse<F: Frontend>(
    frontend: &F,
    source: &str,
    options: &MarkdownOptions,
) -> ParsedDocument {
    if source.is_empty() {
        return ParsedDocument::default();
    }
    let source = normalize_line_endings(source);
    let parse_fragment = |fragment: &str| {
        let mut document = frontend.parse_document(fragment);
        if !F::NATIVE_MARK {
            mark::apply(&mut document);
        }
        document
    };
    let mut document = parse_fragment(&source);
    let lines: Vec<&str> = source.split('\n').collect();
    grid_table::apply(&mut document, &lines, &parse_fragment);
    let mut result = render::render(&document, options, source.len().saturating_mul(2));
    if let std::borrow::Cow::Owned(html) = alerts::transform_gfm_alerts(&result.rendered_body_html)
    {
        result.rendered_body_html = html;
    }
    result
}
