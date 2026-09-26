//! comrak candidate: translates the comrak AST into the Marknexia document
//! model. Rendering and every Markdig-specific behavior live in `extensions`.

use crate::{
    MarkdownEngine, MarkdownOptions, ParsedDocument,
    extensions::{
        self, Frontend,
        model::{
            Alignment, Block, Document, FootnoteDefinition, Inline, LineIndex, LinkKind, List,
            ListItem, Paragraph, Table, TableRow,
        },
    },
};
use comrak::{
    Arena, Options,
    nodes::{AstNode, ListType, NodeValue, TableAlignment},
};
use marknexia_core::contracts::Diagnostic;

/// Exploratory candidate. Output is content-unsafe/unbounded; never pass it to
/// WebView without sanitization and output limits.
pub struct ComrakAdapter;

impl MarkdownEngine for ComrakAdapter {
    fn parse(
        &self,
        source: &str,
        options: &MarkdownOptions,
    ) -> Result<ParsedDocument, Vec<Diagnostic>> {
        Ok(extensions::parse(self, source, options))
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
        };
        let blocks = translator.blocks(root);
        Document {
            blocks,
            footnotes: translator.footnotes,
        }
    }
}

struct Translator<'s> {
    source: &'s str,
    lines: LineIndex,
    footnotes: Vec<FootnoteDefinition>,
}

impl Translator<'_> {
    fn blocks<'a>(&mut self, parent: &'a AstNode<'a>) -> Vec<Block> {
        let mut blocks = Vec::new();
        for node in parent.children() {
            let data = node.data.borrow();
            let line = data.sourcepos.start.line.saturating_sub(1);
            let block = match &data.value {
                NodeValue::Paragraph => Block::Paragraph(Paragraph {
                    line,
                    end_line: data.sourcepos.end.line.saturating_sub(1).max(line),
                    inlines: self.inlines(node),
                }),
                NodeValue::Heading(heading) => Block::Heading {
                    level: heading.level,
                    line,
                    inlines: self.inlines(node),
                },
                NodeValue::ThematicBreak => Block::ThematicBreak,
                NodeValue::BlockQuote => Block::Quote(self.blocks(node)),
                NodeValue::List(list) => {
                    let items = node
                        .children()
                        .map(|item| {
                            let task = match &item.data.borrow().value {
                                NodeValue::TaskItem(task) => Some(task.symbol.is_some()),
                                _ => None,
                            };
                            ListItem {
                                task,
                                blocks: self.blocks(item),
                            }
                        })
                        .collect();
                    Block::List(List {
                        start: (list.list_type == ListType::Ordered).then_some(list.start as u64),
                        tight: list.tight,
                        items,
                    })
                }
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
                NodeValue::FootnoteDefinition(definition) => {
                    let label = definition.name.clone();
                    let blocks = self.blocks(node);
                    self.footnotes.push(FootnoteDefinition { label, blocks });
                    continue;
                }
                _ => {
                    // Unenabled or container-only syntax: keep its block children.
                    drop(data);
                    blocks.extend(self.blocks(node));
                    continue;
                }
            };
            blocks.push(block);
        }
        blocks
    }

    fn row<'a>(&mut self, row: &'a AstNode<'a>) -> TableRow {
        let data = row.data.borrow();
        let line = data.sourcepos.start.line.saturating_sub(1);
        TableRow {
            header: matches!(data.value, NodeValue::TableRow(true)),
            cells: row
                .children()
                .map(|cell| {
                    vec![Block::Paragraph(Paragraph {
                        line,
                        end_line: line,
                        inlines: self.inlines(cell),
                    })]
                })
                .collect(),
        }
    }

    fn inlines<'a>(&mut self, parent: &'a AstNode<'a>) -> Vec<Inline> {
        let mut inlines = Vec::new();
        for node in parent.children() {
            let data = node.data.borrow();
            let inline = match &data.value {
                NodeValue::Text(text) => Inline::Text(text.to_string()),
                NodeValue::SoftBreak => Inline::SoftBreak,
                NodeValue::LineBreak => Inline::HardBreak,
                NodeValue::Code(code) => Inline::Code(code.literal.clone()),
                NodeValue::HtmlInline(html) => Inline::Html(html.clone()),
                NodeValue::Emph => Inline::Emphasis(self.inlines(node)),
                NodeValue::Strong => Inline::Strong(self.inlines(node)),
                NodeValue::Strikethrough => Inline::Strikethrough(self.inlines(node)),
                NodeValue::Highlight => Inline::Mark(self.inlines(node)),
                NodeValue::Superscript => Inline::Superscript(self.inlines(node)),
                NodeValue::Subscript => Inline::Subscript(self.inlines(node)),
                NodeValue::Insert => Inline::Inserted(self.inlines(node)),
                NodeValue::Link(link) => {
                    let start = self
                        .lines
                        .offset(data.sourcepos.start.line, data.sourcepos.start.column);
                    let angle = start
                        .and_then(|offset| self.source.as_bytes().get(offset))
                        .is_some_and(|byte| *byte == b'<');
                    Inline::Link {
                        kind: if angle {
                            LinkKind::Autolink
                        } else {
                            LinkKind::Standard
                        },
                        url: link.url.clone(),
                        title: link.title.clone(),
                        children: self.inlines(node),
                    }
                }
                NodeValue::Image(link) => Inline::Image {
                    url: link.url.clone(),
                    title: link.title.clone(),
                    children: self.inlines(node),
                },
                NodeValue::FootnoteReference(reference) => {
                    Inline::FootnoteReference(reference.name.clone())
                }
                NodeValue::Math(math) => Inline::Math(math.literal.clone()),
                _ => {
                    drop(data);
                    inlines.extend(self.inlines(node));
                    continue;
                }
            };
            inlines.push(inline);
        }
        inlines
    }
}
