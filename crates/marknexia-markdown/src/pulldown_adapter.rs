//! pulldown-cmark candidate: folds the pulldown event stream into the
//! Marknexia document model. Rendering and every Markdig-specific behavior
//! live in `extensions`.

use crate::{
    MarkdownEngine, MarkdownOptions, ParsedDocument,
    extensions::{
        self, Frontend,
        model::{
            Alignment, Block, DepthLimits, Document, FootnoteDefinition, Inline, LineIndex,
            LinkKind, List, ListItem, MAX_BLOCK_DEPTH, MAX_INLINE_DEPTH, Paragraph, Table,
            TableRow,
        },
    },
};
use marknexia_core::contracts::Diagnostic;
use pulldown_cmark::{
    Alignment as PulldownAlignment, CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd,
};
use std::ops::Range;

/// Exploratory candidate. Output is content-unsafe; never pass it to WebView
/// without sanitization.
pub struct PulldownAdapter;

impl MarkdownEngine for PulldownAdapter {
    fn parse(
        &self,
        source: &str,
        options: &MarkdownOptions,
    ) -> Result<ParsedDocument, Vec<Diagnostic>> {
        extensions::parse(self, source, options)
    }
}

impl Frontend for PulldownAdapter {
    const NATIVE_MARK: bool = false;

    fn parse_document(&self, source: &str) -> Document {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_MATH
            | Options::ENABLE_SUPERSCRIPT
            | Options::ENABLE_SUBSCRIPT;
        let mut builder = Builder {
            source,
            lines: LineIndex::new(source),
            stack: vec![Frame::container(Container::Document)],
            footnotes: Vec::new(),
            markers: Vec::new(),
            block_depth: 0,
            span_depth: 0,
            suppressed_items: 0,
            limits: DepthLimits::default(),
        };
        for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
            builder.event(event, range);
        }
        let blocks = match builder.stack.pop() {
            Some(Frame::Container { mut state, .. }) => state.finish(),
            _ => Vec::new(),
        };
        Document {
            blocks,
            footnotes: builder.footnotes,
            diagnostics: builder.limits.diagnostics,
        }
    }
}

enum Container {
    Document,
    BlockQuote,
    Item,
    Footnote(String),
}

#[derive(Default)]
struct ContainerState {
    blocks: Vec<Block>,
    /// Inline events directly inside an item of a tight list.
    implicit: Option<Paragraph>,
    explicit_paragraph: bool,
    task: Option<bool>,
}

impl ContainerState {
    fn flush(&mut self) {
        if let Some(paragraph) = self.implicit.take() {
            self.blocks.push(Block::Paragraph(paragraph));
        }
    }

    fn finish(&mut self) -> Vec<Block> {
        self.flush();
        std::mem::take(&mut self.blocks)
    }
}

enum Span {
    Emphasis,
    Strong,
    Strikethrough,
    Superscript,
    Subscript,
    Link {
        kind: LinkKind,
        url: String,
        title: String,
    },
    Image {
        url: String,
        title: String,
    },
}

enum Frame {
    Container {
        kind: Container,
        state: ContainerState,
    },
    Leaf {
        heading: Option<u8>,
        line: usize,
        end_line: usize,
        inlines: Vec<Inline>,
    },
    Span {
        kind: Span,
        inlines: Vec<Inline>,
    },
    List {
        start: Option<u64>,
        loose: bool,
        items: Vec<ListItem>,
    },
    Table {
        alignments: Vec<Alignment>,
        rows: Vec<TableRow>,
        row: Option<TableRow>,
    },
    Code {
        info: Option<String>,
        literal: String,
        line: usize,
    },
    Html {
        literal: String,
        line: usize,
    },
}

impl Frame {
    fn container(kind: Container) -> Self {
        Self::Container {
            kind,
            state: ContainerState::default(),
        }
    }
}

/// One entry per open `Start` event, so each `End` knows whether its `Start`
/// was kept or suppressed by the depth limits.
enum Marker {
    Kept,
    Suppressed { item: bool },
}

struct Builder<'s> {
    source: &'s str,
    lines: LineIndex,
    stack: Vec<Frame>,
    footnotes: Vec<FootnoteDefinition>,
    markers: Vec<Marker>,
    /// Open quote, list, item, and footnote frames.
    block_depth: usize,
    /// Open span frames.
    span_depth: usize,
    /// Open items whose list was suppressed.
    suppressed_items: usize,
    limits: DepthLimits,
}

fn is_span(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

impl Builder<'_> {
    /// Whether a `Start` must be suppressed to keep the model within
    /// `MAX_BLOCK_DEPTH`/`MAX_INLINE_DEPTH`. Content of a suppressed container
    /// flows into the nearest kept container; a suppressed span's content
    /// flows into its parent inline list.
    fn suppress(&mut self, tag: &Tag<'_>, range: &Range<usize>) -> Option<Marker> {
        let suppressed = match tag {
            Tag::BlockQuote(_) | Tag::FootnoteDefinition(_) => self.block_depth >= MAX_BLOCK_DEPTH,
            // A kept list needs room for its items.
            Tag::List(_) => self.block_depth + 1 >= MAX_BLOCK_DEPTH,
            Tag::Item => matches!(self.markers.last(), Some(Marker::Suppressed { .. })),
            _ if is_span(tag) => {
                if self.span_depth >= MAX_INLINE_DEPTH {
                    self.limits
                        .inline_flattened(self.lines.line_of(range.start));
                    return Some(Marker::Suppressed { item: false });
                }
                false
            }
            _ => false,
        };
        if !suppressed {
            return None;
        }
        self.limits.block_flattened(self.lines.line_of(range.start));
        let item = matches!(tag, Tag::Item);
        if item {
            self.suppressed_items += 1;
        }
        Some(Marker::Suppressed { item })
    }

    fn line(&self, range: &Range<usize>) -> usize {
        self.lines.line_of(range.start)
    }

    fn end_line(&self, range: &Range<usize>) -> usize {
        self.lines
            .line_of(range.end.saturating_sub(1).max(range.start))
    }

    fn push_block(&mut self, block: Block) {
        if let Some(Frame::Container { state, .. }) = self.stack.last_mut() {
            state.flush();
            state.blocks.push(block);
        }
    }

    /// Appends raw text to an open code or HTML block; returns `false` otherwise.
    fn push_literal(&mut self, text: &str) -> bool {
        match self.stack.last_mut() {
            Some(Frame::Code { literal, .. } | Frame::Html { literal, .. }) => {
                literal.push_str(text);
                true
            }
            _ => false,
        }
    }

    fn push_inline(&mut self, inline: Inline, range: &Range<usize>) {
        let lines = &self.lines;
        match self.stack.last_mut() {
            Some(Frame::Leaf { inlines, .. } | Frame::Span { inlines, .. }) => inlines.push(inline),
            Some(Frame::Container { state, .. }) => state
                .implicit
                .get_or_insert_with(|| implicit_paragraph(lines, range))
                .inlines
                .push(inline),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: &Range<usize>) {
        let line = self.line(range);
        let frame = match tag {
            Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell => {
                if let (
                    Tag::Paragraph,
                    Some(Frame::Container {
                        kind: Container::Item,
                        state,
                    }),
                ) = (&tag, self.stack.last_mut())
                {
                    state.explicit_paragraph = true;
                }
                if let Some(Frame::Container { state, .. }) = self.stack.last_mut() {
                    state.flush();
                }
                Frame::Leaf {
                    heading: match tag {
                        Tag::Heading { level, .. } => Some(level as u8),
                        _ => None,
                    },
                    line,
                    end_line: self.end_line(range),
                    inlines: Vec::new(),
                }
            }
            Tag::BlockQuote(_) => Frame::container(Container::BlockQuote),
            Tag::Item => Frame::container(Container::Item),
            Tag::FootnoteDefinition(label) => {
                Frame::container(Container::Footnote(label.into_string()))
            }
            Tag::List(start) => Frame::List {
                start,
                loose: false,
                items: Vec::new(),
            },
            Tag::Table(alignments) => Frame::Table {
                alignments: alignments
                    .into_iter()
                    .map(|alignment| match alignment {
                        PulldownAlignment::None => Alignment::None,
                        PulldownAlignment::Left => Alignment::Left,
                        PulldownAlignment::Center => Alignment::Center,
                        PulldownAlignment::Right => Alignment::Right,
                    })
                    .collect(),
                rows: Vec::new(),
                row: None,
            },
            Tag::TableHead | Tag::TableRow => {
                if let Some(Frame::Table { row, .. }) = self.stack.last_mut() {
                    *row = Some(TableRow {
                        header: matches!(tag, Tag::TableHead),
                        cells: Vec::new(),
                    });
                }
                return;
            }
            Tag::CodeBlock(kind) => Frame::Code {
                info: match kind {
                    CodeBlockKind::Fenced(info) => Some(info.into_string()),
                    CodeBlockKind::Indented => None,
                },
                literal: String::new(),
                line,
            },
            Tag::HtmlBlock => Frame::Html {
                literal: String::new(),
                line,
            },
            Tag::Emphasis => Frame::Span {
                kind: Span::Emphasis,
                inlines: Vec::new(),
            },
            Tag::Strong => Frame::Span {
                kind: Span::Strong,
                inlines: Vec::new(),
            },
            Tag::Strikethrough => Frame::Span {
                kind: Span::Strikethrough,
                inlines: Vec::new(),
            },
            Tag::Superscript => Frame::Span {
                kind: Span::Superscript,
                inlines: Vec::new(),
            },
            Tag::Subscript => Frame::Span {
                kind: Span::Subscript,
                inlines: Vec::new(),
            },
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => {
                let (kind, url) = match link_type {
                    LinkType::Autolink => (LinkKind::Autolink, dest_url.into_string()),
                    LinkType::Email => (LinkKind::Autolink, format!("mailto:{dest_url}")),
                    _ => (LinkKind::Standard, dest_url.into_string()),
                };
                Frame::Span {
                    kind: Span::Link {
                        kind,
                        url,
                        title: title.into_string(),
                    },
                    inlines: Vec::new(),
                }
            }
            Tag::Image {
                dest_url, title, ..
            } => Frame::Span {
                kind: Span::Image {
                    url: dest_url.into_string(),
                    title: title.into_string(),
                },
                inlines: Vec::new(),
            },
            // Not enabled: keep children by treating them as a transparent span.
            Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => return,
        };
        self.stack.push(frame);
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(Frame::Table { rows, row, .. }) = self.stack.last_mut() {
                    rows.extend(row.take());
                }
                return;
            }
            TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => return,
            _ => {}
        }
        let Some(frame) = self.stack.pop() else {
            return;
        };
        match &frame {
            Frame::Container { .. } | Frame::List { .. } => self.block_depth -= 1,
            Frame::Span { .. } => self.span_depth -= 1,
            _ => {}
        }
        match frame {
            Frame::Leaf {
                heading,
                line,
                end_line,
                inlines,
            } => {
                if matches!(tag, TagEnd::TableCell) {
                    if let Some(Frame::Table { row: Some(row), .. }) = self.stack.last_mut() {
                        row.cells.push(vec![Block::Paragraph(Paragraph {
                            line,
                            end_line,
                            inlines,
                        })]);
                    }
                    return;
                }
                self.push_block(match heading {
                    Some(level) => Block::Heading {
                        level,
                        line,
                        inlines,
                    },
                    None => Block::Paragraph(Paragraph {
                        line,
                        end_line,
                        inlines,
                    }),
                });
            }
            Frame::Container { kind, mut state } => {
                let blocks = state.finish();
                match kind {
                    Container::Document => {}
                    Container::BlockQuote => self.push_block(Block::Quote(blocks)),
                    Container::Item => {
                        if let Some(Frame::List { loose, items, .. }) = self.stack.last_mut() {
                            *loose |= state.explicit_paragraph;
                            items.push(ListItem {
                                task: state.task,
                                blocks,
                            });
                        }
                    }
                    Container::Footnote(label) => {
                        self.footnotes.push(FootnoteDefinition { label, blocks });
                    }
                }
            }
            Frame::List {
                start,
                loose,
                items,
            } => self.push_block(Block::List(List {
                start,
                tight: !loose,
                items,
            })),
            Frame::Table {
                alignments, rows, ..
            } => self.push_block(Block::Table(Table {
                alignments,
                widths: Vec::new(),
                rows,
            })),
            Frame::Code {
                info,
                literal,
                line,
            } => self.push_block(Block::Code {
                info,
                literal,
                line,
            }),
            Frame::Html { literal, line } => self.push_block(Block::Html { literal, line }),
            Frame::Span { kind, inlines } => {
                let inline = match kind {
                    Span::Emphasis => Inline::Emphasis(inlines),
                    Span::Strong => Inline::Strong(inlines),
                    Span::Strikethrough => Inline::Strikethrough(inlines),
                    Span::Superscript => Inline::Superscript(inlines),
                    Span::Subscript => Inline::Subscript(inlines),
                    Span::Link { kind, url, title } => Inline::Link {
                        kind,
                        url,
                        title,
                        children: inlines,
                    },
                    Span::Image { url, title } => Inline::Image {
                        url,
                        title,
                        children: inlines,
                    },
                };
                self.push_inline_to_parent(inline);
            }
        }
    }

    /// Spans close without a range of their own; they never start an implicit
    /// paragraph because their first child already did.
    fn push_inline_to_parent(&mut self, inline: Inline) {
        match self.stack.last_mut() {
            Some(Frame::Leaf { inlines, .. } | Frame::Span { inlines, .. }) => inlines.push(inline),
            Some(Frame::Container { state, .. }) => {
                if let Some(paragraph) = state.implicit.as_mut() {
                    paragraph.inlines.push(inline);
                }
            }
            _ => {}
        }
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => {
                if let Some(marker) = self.suppress(&tag, &range) {
                    self.markers.push(marker);
                    return;
                }
                self.markers.push(Marker::Kept);
                // Spans directly in a tight item must open the implicit paragraph.
                if is_span(&tag) {
                    self.open_implicit(&range);
                }
                let depth = self.stack.len();
                self.start(tag, &range);
                if self.stack.len() > depth {
                    match self.stack.last() {
                        Some(Frame::Container { .. } | Frame::List { .. }) => self.block_depth += 1,
                        Some(Frame::Span { .. }) => self.span_depth += 1,
                        _ => {}
                    }
                }
            }
            Event::End(tag) => match self.markers.pop() {
                Some(Marker::Suppressed { item }) => {
                    if item {
                        self.suppressed_items -= 1;
                    }
                }
                _ => self.end(tag),
            },
            Event::Text(text) => {
                if self.push_literal(&text) {
                    return;
                }
                // An escaped character starts its own event right after the
                // backslash, and an entity's event starts at `&`. Keep such an
                // `=` out of `==mark==` delimiter runs, as Markdig does.
                let escaped = text.starts_with('=')
                    && (!self.source[range.start..].starts_with('=')
                        || self.source.as_bytes()[..range.start]
                            .iter()
                            .rev()
                            .take_while(|byte| **byte == b'\\')
                            .count()
                            % 2
                            == 1);
                if escaped {
                    self.push_inline(Inline::Escaped("=".to_owned()), &range);
                    if text.len() > 1 {
                        self.push_inline(Inline::Text(text[1..].to_owned()), &range);
                    }
                } else {
                    self.push_inline(Inline::Text(text.into_string()), &range);
                }
            }
            Event::Code(code) => self.push_inline(Inline::Code(code.into_string()), &range),
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                self.push_inline(Inline::Math(math.into_string()), &range);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                if !self.push_literal(&html) {
                    self.push_inline(Inline::Html(html.into_string()), &range);
                }
            }
            Event::FootnoteReference(label) => {
                self.push_inline(Inline::FootnoteReference(label.into_string()), &range);
            }
            Event::SoftBreak => self.push_inline(Inline::SoftBreak, &range),
            Event::HardBreak => self.push_inline(Inline::HardBreak, &range),
            Event::Rule => self.push_block(Block::ThematicBreak),
            // A suppressed item's marker must not mark its kept ancestor.
            Event::TaskListMarker(_) if self.suppressed_items > 0 => {}
            Event::TaskListMarker(checked) => {
                let item = self.stack.iter_mut().rev().find_map(|frame| match frame {
                    Frame::Container {
                        kind: Container::Item,
                        state,
                    } => Some(state),
                    _ => None,
                });
                if let Some(state) = item {
                    state.task = Some(checked);
                }
            }
        }
    }

    fn open_implicit(&mut self, range: &Range<usize>) {
        let lines = &self.lines;
        if let Some(Frame::Container { state, .. }) = self.stack.last_mut() {
            state
                .implicit
                .get_or_insert_with(|| implicit_paragraph(lines, range));
        }
    }
}

/// Implicit paragraphs only occur inside tight list items, which grid-table
/// detection never inspects, so the paragraph is attributed to its first line.
fn implicit_paragraph(lines: &LineIndex, range: &Range<usize>) -> Paragraph {
    let line = lines.line_of(range.start);
    Paragraph {
        line,
        end_line: line,
        inlines: Vec::new(),
    }
}
