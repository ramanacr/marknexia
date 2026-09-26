#![forbid(unsafe_code)]

//! Markdown parser bake-off; no parser is selected here.
//!
//! Candidate adapters translate third-party syntax trees into a
//! Marknexia-owned model; `extensions` reproduces the .NET Markdig 0.40
//! pipeline on top of it. Output is content-unsafe: it includes unsanitized
//! raw HTML (including parity-required mis-nested alert markup) and must never
//! reach WebView before an HTML5-parser-based sanitizer. Nesting depth and
//! rendered size are bounded (`MarkdownOptions`, see
//! `compat/decisions/markdown-resource-limits.md`).

mod engine;
#[cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]
mod extensions;
pub use engine::{Anchor, Diagram, Heading, MarkdownEngine, MarkdownOptions, ParsedDocument};

#[cfg(feature = "candidate-comrak")]
mod comrak_adapter;
#[cfg(feature = "candidate-comrak")]
pub use comrak_adapter::ComrakAdapter;

#[cfg(feature = "candidate-pulldown")]
mod pulldown_adapter;
#[cfg(feature = "candidate-pulldown")]
pub use pulldown_adapter::PulldownAdapter;
