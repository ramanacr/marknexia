#![forbid(unsafe_code)]

//! Exploratory parser bake-off; no parser is selected here.
//! Candidate output is content-unsafe and unbounded. It includes unsanitized raw
//! HTML and must never be sent to WebView before sanitization and output limits.

#[cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]
mod compatibility;
mod engine;
pub use engine::{Anchor, Diagram, Heading, MarkdownEngine, MarkdownOptions, ParsedDocument};

#[cfg(feature = "candidate-comrak")]
mod comrak_adapter;
#[cfg(feature = "candidate-comrak")]
pub use comrak_adapter::ComrakAdapter;

#[cfg(feature = "candidate-pulldown")]
mod pulldown_adapter;
#[cfg(feature = "candidate-pulldown")]
pub use pulldown_adapter::PulldownAdapter;
