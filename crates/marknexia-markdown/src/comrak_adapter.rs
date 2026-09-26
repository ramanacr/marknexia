//! comrak candidate: translates the comrak AST into the Marknexia document
//! model. Rendering and every Markdig-specific behavior live in `extensions`.

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
use comrak::{
    Arena, Options,
    nodes::{AstNode, ListType, NodeValue, TableAlignment},
};
use marknexia_core::contracts::Diagnostic;

/// Exploratory candidate. Output is content-unsafe; never pass it to WebView
/// without sanitization.
pub struct ComrakAdapter;

impl MarkdownEngine for ComrakAdapter {
    fn parse(
        &self,
        source: &str,
        options: &MarkdownOptions,
    ) -> Result<ParsedDocument, Vec<Diagnostic>> {
        extensions::parse(self, source, options)
    }
}

impl Frontend for ComrakAdapter {
    const NATIVE_MARK: bool = true;

    fn parse_document(&self, source: &str) -> Document {
        let mut options = Options::default();
        let extension = &mut options.extension;
        extension.table = true;
        extension.tasklist = true;
        extension.strikethrough = true;
        extension.footnotes = true;
        extension.autolink = true;
        extension.math_dollars = true;
        extension.highlight = true;
        extension.superscript = true;
        extension.subscript = true;
        extension.insert = true;
        let arena = Arena::new();
        let root = comrak::parse_document(&arena, source, &options);
        let mut translator = Translator {
            source,
            lines: LineIndex::new(source),
            footnotes: Vec::new(),
            limits: DepthLimits::default(),
        };
        let blocks = translator.blocks(root, 0);
        Document {
            blocks,
            footnotes: translator.footnotes,
            diagnostics: translator.limits.diagnostics,
        }
    }
}

struct Translator<'s> {
    source: &'s str,
    lines: LineIndex,
    footnotes: Vec<FootnoteDefinition>,
    limits: DepthLimits,
}

fn start_line<'a>(node: &'a AstNode<'a>) -> usize {
    node.data.borrow().sourcepos.start.line.saturating_sub(1)
}

impl Translator<'_> {
    /// Leaf blocks, which never contain block containers.
    fn leaf<'a>(&mut self, node: &'a AstNode<'a>) -> Option<Block> {
        let data = node.data.borrow();
        let line = data.sourcepos.start.line.saturating_sub(1);
        Some(match &data.value {
            NodeValue::Paragraph => Block::Paragraph(Paragraph {
                line,
                end_line: data.sourcepos.end.line.saturating_sub(1).max(line),
                inlines: self.inlines(node, 0),
            }),
            NodeValue::Heading(heading) => Block::Heading {
                level: heading.level,
                line,
                inlines: self.inlines(node, 0),
            },
            NodeValue::ThematicBreak => Block::ThematicBreak,
            NodeValue::CodeBlock(code) => Block::Code {
                info: code.fenced.then(|| code.info.clone()),
                literal: code.literal.clone(),
                line,
            },
            NodeValue::HtmlBlock(html) => Block::Html {
                literal: html.literal.clone(),
                line,
            },
            NodeValue::Table(table) => Block::Table(Table {
                alignments: table
                    .alignments
                    .iter()
                    .map(|alignment| match alignment {
                        TableAlignment::None => Alignment::None,
                        TableAlignment::Left => Alignment::Left,
                        TableAlignment::Center => Alignment::Center,
                        TableAlignment::Right => Alignment::Right,
                    })
                    .collect(),
                widths: Vec::new(),
                rows: node.children().map(|row| self.row(row)).collect(),
            }),
            _ => return None,
        })
    }

    /// `depth` counts kept block containers above `parent`'s children. A
    /// container that would exceed `MAX_BLOCK_DEPTH` is flattened: its leaf
    /// blocks are lifted into `parent`.
    fn blocks<'a>(&mut self, parent: &'a AstNode<'a>, depth: usize) -> Vec<Block> {
        let mut blocks = Vec::new();
        for node in parent.children() {
            if let Some(block) = self.leaf(node) {
                blocks.push(block);
                continue;
            }
            let value = node.data.borrow().value.clone();
            match value {
                NodeValue::BlockQuote if depth < MAX_BLOCK_DEPTH => {
                    blocks.push(Block::Quote(self.blocks(node, depth + 1)));
                }
                // A kept list needs room for its items.
                NodeValue::List(list) if depth + 1 < MAX_BLOCK_DEPTH => {
                    let items = node
                        .children()
                        .map(|item| {
                            let task = match &item.data.borrow().value {
                                NodeValue::TaskItem(task) => Some(task.symbol.is_some()),
                                _ => None,
                            };
                            ListItem {
                                task,
                                blocks: self.blocks(item, depth + 2),
                            }
                        })
                        .collect();
                    blocks.push(Block::List(List {
                        start: (list.list_type == ListType::Ordered).then_some(list.start as u64),
                        tight: list.tight,
                        items,
                    }));
                }
                NodeValue::FootnoteDefinition(definition) if depth < MAX_BLOCK_DEPTH => {
                    let blocks = self.blocks(node, depth + 1);
                    self.footnotes.push(FootnoteDefinition {
                        label: definition.name,
                        blocks,
                    });
                }
                // Unenabled or container-only syntax: keep its block children.
                NodeValue::BlockQuote | NodeValue::List(_) | NodeValue::FootnoteDefinition(_) => {
                    self.limits.block_flattened(start_line(node));
                    self.flatten_blocks(node, &mut blocks);
                }
                _ if depth < MAX_BLOCK_DEPTH => blocks.extend(self.blocks(node, depth + 1)),
                _ => {
                    self.limits.block_flattened(start_line(node));
                    self.flatten_blocks(node, &mut blocks);
                }
            }
        }
        blocks
    }

    /// Iteratively lifts every leaf block below `node` into `blocks`.
    fn flatten_blocks<'a>(&mut self, node: &'a AstNode<'a>, blocks: &mut Vec<Block>) {
        for descendant in node.descendants().skip(1) {
            let is_leaf = matches!(
                descendant.data.borrow().value,
                NodeValue::Paragraph
                    | NodeValue::Heading(_)
                    | NodeValue::ThematicBreak
                    | NodeValue::CodeBlock(_)
                    | NodeValue::HtmlBlock(_)
                    | NodeValue::Table(_)
            );
            if is_leaf && let Some(block) = self.leaf(descendant) {
                blocks.push(block);
            }
        }
    }

    fn row<'a>(&mut self, row: &'a AstNode<'a>) -> TableRow {
        let (header, line) = {
            let data = row.data.borrow();
            (
                matches!(data.value, NodeValue::TableRow(true)),
                data.sourcepos.start.line.saturating_sub(1),
            )
        };
        TableRow {
            header,
            cells: row
                .children()
                .map(|cell| {
                    vec![Block::Paragraph(Paragraph {
                        line,
                        end_line: line,
                        inlines: self.inlines(cell, 0),
                    })]
                })
                .collect(),
        }
    }

    /// Iteratively collects the text below an inline container that is too
    /// deep to keep.
    fn flatten_inline<'a>(&mut self, node: &'a AstNode<'a>) -> Inline {
        self.limits.inline_flattened(start_line(node));
        let mut text = String::new();
        for descendant in node.descendants().skip(1) {
            match &descendant.data.borrow().value {
                NodeValue::Text(value) => text.push_str(value),
                NodeValue::Code(code) => text.push_str(&code.literal),
                NodeValue::Math(math) => text.push_str(&math.literal),
                NodeValue::SoftBreak | NodeValue::LineBreak => text.push('\n'),
                _ => {}
            }
        }
        Inline::Text(text)
    }

    /// `depth` counts kept inline containers above `parent`'s children.
    fn inlines<'a>(&mut self, parent: &'a AstNode<'a>, depth: usize) -> Vec<Inline> {
        let mut inlines = Vec::new();
        for node in parent.children() {
            let (value, start) = {
                let data = node.data.borrow();
                let leaf = match &data.value {
                    NodeValue::Text(text) => Some(Inline::Text(text.to_string())),
                    NodeValue::SoftBreak => Some(Inline::SoftBreak),
                    NodeValue::LineBreak => Some(Inline::HardBreak),
                    NodeValue::Code(code) => Some(Inline::Code(code.literal.clone())),
                    NodeValue::HtmlInline(html) => Some(Inline::Html(html.clone())),
                    NodeValue::FootnoteReference(reference) => {
                        Some(Inline::FootnoteReference(reference.name.clone()))
                    }
                    NodeValue::Math(math) => Some(Inline::Math(math.literal.clone())),
                    _ => None,
                };
                if let Some(leaf) = leaf {
                    inlines.push(leaf);
                    continue;
                }
                (
                    data.value.clone(),
                    (data.sourcepos.start.line, data.sourcepos.start.column),
                )
            };
            if depth >= MAX_INLINE_DEPTH {
                inlines.push(self.flatten_inline(node));
                continue;
            }
            let children = self.inlines(node, depth + 1);
            inlines.push(match value {
                NodeValue::Emph => Inline::Emphasis(children),
                NodeValue::Strong => Inline::Strong(children),
                NodeValue::Strikethrough => Inline::Strikethrough(children),
                NodeValue::Highlight => Inline::Mark(children),
                NodeValue::Superscript => Inline::Superscript(children),
                NodeValue::Subscript => Inline::Subscript(children),
                NodeValue::Insert => Inline::Inserted(children),
                NodeValue::Link(link) => {
                    let angle = self
                        .lines
                        .offset(start.0, start.1)
                        .and_then(|offset| self.source.as_bytes().get(offset))
                        .is_some_and(|byte| *byte == b'<');
                    Inline::Link {
                        kind: if angle {
                            LinkKind::Autolink
                        } else {
                            LinkKind::Standard
                        },
                        url: link.url,
                        title: link.title,
                        children,
                    }
                }
                NodeValue::Image(link) => Inline::Image {
                    url: link.url,
                    title: link.title,
                    children,
                },
                // Unenabled inline containers keep their children.
                _ => {
                    inlines.extend(children);
                    continue;
                }
            });
        }
        inlines
    }
}
