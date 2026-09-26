//! Marknexia-owned document model. Parser adapters translate third-party syntax
//! trees into this model so that Markdig-compatible rendering, metadata
//! extraction, and compatibility extensions never depend on one library.

use marknexia_core::contracts::{Diagnostic, DiagnosticSeverity};

/// Deepest container nesting (quotes, lists, items, footnotes) an adapter
/// builds. Deeper containers are flattened into their nearest kept ancestor so
/// every recursive walk over the model, including `Drop`, stays within a small
/// fixed stack budget. Markdig has no equivalent limit; see the decision doc.
pub(crate) const MAX_BLOCK_DEPTH: usize = 32;

/// Deepest inline container nesting (emphasis, links, images, marks, ...).
/// Deeper spans keep their content but lose the span.
pub(crate) const MAX_INLINE_DEPTH: usize = 32;

/// A parsed document before Marknexia extensions and rendering run.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Document {
    pub blocks: Vec<Block>,
    /// Footnote definitions keyed by their label as written.
    pub footnotes: Vec<FootnoteDefinition>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Records at most one flattening diagnostic per kind for a document.
#[derive(Default)]
pub(crate) struct DepthLimits {
    block_reported: bool,
    inline_reported: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl DepthLimits {
    pub(crate) fn block_flattened(&mut self, line: usize) {
        if !std::mem::replace(&mut self.block_reported, true) {
            self.diagnostics
                .push(depth_diagnostic("block", MAX_BLOCK_DEPTH, line));
        }
    }

    pub(crate) fn inline_flattened(&mut self, line: usize) {
        if !std::mem::replace(&mut self.inline_reported, true) {
            self.diagnostics
                .push(depth_diagnostic("inline", MAX_INLINE_DEPTH, line));
        }
    }
}

fn depth_diagnostic(kind: &str, limit: usize, line: usize) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Warning,
        message: format!("{kind} nesting deeper than {limit} levels was flattened"),
        source_line: Some(line),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FootnoteDefinition {
    pub label: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    Paragraph(Paragraph),
    Heading {
        level: u8,
        line: usize,
        inlines: Vec<Inline>,
    },
    ThematicBreak,
    Quote(Vec<Block>),
    List(List),
    Code {
        /// The complete info string for fenced blocks; `None` for indented blocks.
        info: Option<String>,
        /// Code lines, each terminated by `\n`.
        literal: String,
        line: usize,
    },
    Html {
        literal: String,
        line: usize,
    },
    Table(Table),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Paragraph {
    /// Zero-based first source line.
    pub line: usize,
    /// Zero-based last source line.
    pub end_line: usize,
    pub inlines: Vec<Inline>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct List {
    /// `None` for bullet lists.
    pub start: Option<u64>,
    pub tight: bool,
    pub items: Vec<ListItem>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ListItem {
    /// `Some(checked)` for task-list items.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Alignment {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Table {
    pub alignments: Vec<Alignment>,
    /// Column widths in percent; empty when the syntax has no widths (pipe tables).
    pub widths: Vec<f32>,
    pub rows: Vec<TableRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TableRow {
    pub header: bool,
    pub cells: Vec<Vec<Block>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinkKind {
    /// Inline, reference, or extension (bare URL) links. Markdig `LinkInline`.
    Standard,
    /// `<scheme:...>` or `<user@host>` autolinks. Markdig `AutolinkInline`.
    Autolink,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Inline {
    Text(String),
    /// Backslash-escaped text: renders like `Text` but never forms or merges
    /// into an extension delimiter run (for example `\=` next to `==`).
    #[cfg_attr(not(feature = "candidate-pulldown"), allow(dead_code))]
    Escaped(String),
    Code(String),
    SoftBreak,
    HardBreak,
    Html(String),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Mark(Vec<Inline>),
    Subscript(Vec<Inline>),
    Superscript(Vec<Inline>),
    /// `++text++`; only comrak parses it natively.
    #[cfg_attr(not(feature = "candidate-comrak"), allow(dead_code))]
    Inserted(Vec<Inline>),
    Link {
        kind: LinkKind,
        url: String,
        title: String,
        children: Vec<Inline>,
    },
    Image {
        url: String,
        title: String,
        children: Vec<Inline>,
    },
    FootnoteReference(String),
    Math(String),
}

impl Inline {
    pub(crate) fn children_mut(&mut self) -> Option<&mut Vec<Inline>> {
        match self {
            Self::Emphasis(children)
            | Self::Strong(children)
            | Self::Strikethrough(children)
            | Self::Mark(children)
            | Self::Subscript(children)
            | Self::Superscript(children)
            | Self::Inserted(children)
            | Self::Link { children, .. }
            | Self::Image { children, .. } => Some(children),
            _ => None,
        }
    }
}

/// Zero-based line lookup for byte offsets and 1-based comrak line/column pairs.
pub(crate) struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub(crate) fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index + 1),
        );
        Self { starts }
    }

    #[cfg(any(feature = "candidate-pulldown", test))]
    pub(crate) fn line_of(&self, offset: usize) -> usize {
        self.starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }

    /// Byte offset of a 1-based line and 1-based byte column.
    #[cfg(any(feature = "candidate-comrak", test))]
    pub(crate) fn offset(&self, line: usize, column: usize) -> Option<usize> {
        let start = *self.starts.get(line.checked_sub(1)?)?;
        Some(start + column.checked_sub(1)?)
    }
}

/// Markdig treats `\r\n`, `\r`, and `\n` as line endings and renders `\n`.
pub(crate) fn normalize_line_endings(source: &str) -> std::borrow::Cow<'_, str> {
    if source.contains('\r') {
        std::borrow::Cow::Owned(source.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_maps_offsets_and_columns() {
        let index = LineIndex::new("ab\ncd\n\nef");
        assert_eq!(index.line_of(0), 0);
        assert_eq!(index.line_of(2), 0);
        assert_eq!(index.line_of(3), 1);
        assert_eq!(index.line_of(6), 2);
        assert_eq!(index.line_of(8), 3);
        assert_eq!(index.offset(2, 2), Some(4));
        assert_eq!(index.offset(0, 1), None);
    }

    #[test]
    fn line_endings_normalize_like_markdig() {
        assert_eq!(normalize_line_endings("a\r\nb\rc\n"), "a\nb\nc\n");
        assert!(matches!(
            normalize_line_endings("a\n"),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
