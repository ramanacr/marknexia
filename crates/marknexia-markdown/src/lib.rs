#![forbid(unsafe_code)]

//! Markdown parsing. pulldown-cmark is the selected parser (owner decision
//! 2026-09-28).
//!
//! The adapter translates pulldown-cmark's event stream into a
//! Marknexia-owned model; `extensions` reproduces the .NET Markdig 0.40
//! pipeline on top of it. Output is content-unsafe: it includes unsanitized
//! raw HTML (including parity-required mis-nested alert markup) and must never
//! reach WebView before an HTML5-parser-based sanitizer. Nesting depth and
//! rendered size are bounded (`MarkdownOptions`, see
//! `compat/decisions/markdown-resource-limits.md`).

mod engine;
mod extensions;
pub use engine::{Anchor, Diagram, Heading, MarkdownEngine, MarkdownOptions, ParsedDocument};

mod pulldown_adapter;
pub use pulldown_adapter::PulldownAdapter;
