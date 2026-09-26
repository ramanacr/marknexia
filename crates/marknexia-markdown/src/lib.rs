#![forbid(unsafe_code)]

//! Markdown parser bake-off; no parser is selected here.
//!
//! Candidate adapters translate third-party syntax trees into a
//! Marknexia-owned model; `extensions` reproduces the .NET Markdig 0.40
//! pipeline on top of it. Output is content-unsafe and unbounded: it includes
//! unsanitized raw HTML and must never be sent to WebView before sanitization
//! and output limits.

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
