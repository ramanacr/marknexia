//! Markdig 0.40-compatible HTML writer and metadata extraction over the
//! Marknexia document model. Output is raw, unsanitized, content-unsafe HTML.

use super::{
    anchors::extract_anchors,
    mermaid,
    model::{
        Alignment, Block, Document, FootnoteDefinition, Inline, LinkKind, List, Paragraph, Table,
    },
};
use crate::{Diagram, Heading, MarkdownOptions, ParsedDocument};
use marknexia_core::slug::{SlugSet, generate_heading_slug};
use std::collections::HashMap;

/// Renders `document`, or returns `None` as soon as the output exceeds
/// `options.max_rendered_body_bytes`. Recursion depth is bounded by the
/// adapters' `MAX_BLOCK_DEPTH`/`MAX_INLINE_DEPTH` caps.
pub(crate) fn render(
    document: &Document,
    options: &MarkdownOptions,
    capacity: usize,
) -> Option<ParsedDocument> {
    let footnotes = FootnotePlan::new(document);
    let mut renderer = Renderer {
        out: String::with_capacity(capacity),
        result: ParsedDocument::default(),
        slugs: SlugSet::default(),
        options,
        limit: options.max_rendered_body_bytes,
        references_seen: vec![0; footnotes.ordered.len()],
        footnotes: &footnotes,
        pending_task: None,
    };
    renderer.blocks(&document.blocks, false);
    renderer.footnote_group();
    if renderer.over_limit() {
        return None;
    }
    let mut result = renderer.result;
    result.rendered_body_html = renderer.out;
    Some(result)
}

/// Markdig numbers footnotes by first reference in document order (footnote
/// bodies are processed after the main flow) and numbers every reference link
/// globally in footnote order.
struct FootnotePlan<'d> {
    ordered: Vec<&'d FootnoteDefinition>,
    order: HashMap<String, usize>,
    reference_counts: Vec<usize>,
    link_base: Vec<usize>,
}

impl<'d> FootnotePlan<'d> {
    fn new(document: &'d Document) -> Self {
        let definitions: HashMap<String, &FootnoteDefinition> = document
            .footnotes
            .iter()
            .rev()
            .map(|definition| (normalize_label(&definition.label), definition))
            .collect();
        let mut plan = Self {
            ordered: Vec::new(),
            order: HashMap::new(),
            reference_counts: Vec::new(),
            link_base: Vec::new(),
        };
        let mut labels = Vec::new();
        collect_references(&document.blocks, &mut labels);
        plan.assign(&labels, &definitions);
        let mut next = 0;
        while next < plan.ordered.len() {
            let mut nested = Vec::new();
            collect_references(&plan.ordered[next].blocks, &mut nested);
            plan.assign(&nested, &definitions);
            next += 1;
        }
        let mut base = 0;
        for count in &plan.reference_counts {
            plan.link_base.push(base);
            base += count;
        }
        plan
    }

    fn assign(&mut self, labels: &[String], definitions: &HashMap<String, &'d FootnoteDefinition>) {
        for label in labels {
            let key = normalize_label(label);
            let Some(definition) = definitions.get(&key) else {
                continue;
            };
            let index = *self.order.entry(key).or_insert_with(|| {
                self.ordered.push(definition);
                self.reference_counts.push(0);
                self.ordered.len() - 1
            });
            self.reference_counts[index] += 1;
        }
    }
}

fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn collect_references(blocks: &[Block], labels: &mut Vec<String>) {
    fn inlines(items: &[Inline], labels: &mut Vec<String>) {
        for item in items {
            match item {
                Inline::FootnoteReference(label) => labels.push(label.clone()),
                Inline::Emphasis(children)
                | Inline::Strong(children)
                | Inline::Strikethrough(children)
                | Inline::Mark(children)
                | Inline::Subscript(children)
                | Inline::Superscript(children)
                | Inline::Inserted(children)
                | Inline::Link { children, .. }
                | Inline::Image { children, .. } => inlines(children, labels),
                _ => {}
            }
        }
    }
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => inlines(&paragraph.inlines, labels),
            Block::Heading { inlines: items, .. } => inlines(items, labels),
            Block::Quote(children) => collect_references(children, labels),
            Block::List(list) => {
                for item in &list.items {
                    collect_references(&item.blocks, labels);
                }
            }
            Block::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        collect_references(cell, labels);
                    }
                }
            }
            Block::ThematicBreak | Block::Code { .. } | Block::Html { .. } => {}
        }
    }
}

struct Renderer<'a> {
    out: String,
    result: ParsedDocument,
    slugs: SlugSet,
    options: &'a MarkdownOptions,
    limit: usize,
    footnotes: &'a FootnotePlan<'a>,
    references_seen: Vec<usize>,
    pending_task: Option<bool>,
}

impl Renderer<'_> {
    fn ensure_line(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn over_limit(&self) -> bool {
        self.out.len() > self.limit
    }

    fn blocks(&mut self, blocks: &[Block], implicit: bool) {
        for (index, block) in blocks.iter().enumerate() {
            if self.over_limit() {
                return;
            }
            self.block(block, index == 0, implicit);
        }
    }

    fn block(&mut self, block: &Block, first: bool, implicit: bool) {
        match block {
            Block::Paragraph(paragraph) => self.paragraph(paragraph, first, implicit, ""),
            Block::Heading {
                level,
                line,
                inlines,
            } => self.heading(*level, *line, inlines),
            Block::ThematicBreak => self.out.push_str("<hr />\n"),
            Block::Quote(children) => {
                self.ensure_line();
                self.out.push_str("<blockquote>\n");
                self.blocks(children, false);
                self.out.push_str("</blockquote>\n");
                self.ensure_line();
            }
            Block::List(list) => self.list(list),
            Block::Code {
                info,
                literal,
                line,
            } => self.code(info.as_deref(), literal, *line),
            Block::Html { literal, line } => {
                extract_anchors(literal, *line, &mut self.result.custom_anchors);
                self.out.push_str(literal);
                if !literal.ends_with('\n') {
                    self.out.push('\n');
                }
            }
            Block::Table(table) => self.table(table),
        }
    }

    fn paragraph(&mut self, paragraph: &Paragraph, first: bool, implicit: bool, suffix: &str) {
        self.collect(&paragraph.inlines, paragraph.line);
        if !implicit {
            if !first {
                self.ensure_line();
            }
            self.out.push_str("<p>");
        }
        if let Some(checked) = self.pending_task.take() {
            self.task_marker(checked);
            self.out.push(' ');
        }
        self.inlines(&paragraph.inlines, false);
        self.out.push_str(suffix);
        if !implicit {
            self.out.push_str("</p>\n");
        }
    }

    fn task_marker(&mut self, checked: bool) {
        self.out
            .push_str("<input disabled=\"disabled\" type=\"checkbox\"");
        if checked {
            self.out.push_str(" checked=\"checked\"");
        }
        self.out.push_str(" />");
    }

    fn heading(&mut self, level: u8, line: usize, inlines: &[Inline]) {
        self.collect(inlines, line);
        let mut text = String::new();
        heading_text(inlines, &mut text);
        let text = text.trim().to_owned();
        let slug = generate_heading_slug(&text, &mut self.slugs);
        self.out.push_str("<h");
        self.out.push(char::from(b'0' + level));
        self.out.push_str(" id=\"");
        escape_html(&slug, &mut self.out);
        self.out.push_str("\">");
        self.inlines(inlines, false);
        self.out.push_str("</h");
        self.out.push(char::from(b'0' + level));
        self.out.push_str(">\n");
        self.result.headings.push(Heading {
            text,
            level,
            slug_id: slug,
            line_number: line,
        });
    }

    fn list(&mut self, list: &List) {
        self.ensure_line();
        let tag = if list.start.is_some() { "ol" } else { "ul" };
        self.out.push('<');
        self.out.push_str(tag);
        if let Some(start) = list.start.filter(|start| *start != 1) {
            self.out.push_str(" start=\"");
            self.out.push_str(&start.to_string());
            self.out.push('"');
        }
        if list.items.iter().any(|item| item.task.is_some()) {
            self.out.push_str(" class=\"contains-task-list\"");
        }
        self.out.push_str(">\n");
        for item in &list.items {
            if self.over_limit() {
                break;
            }
            self.ensure_line();
            self.out.push_str("<li");
            if item.task.is_some() {
                self.out.push_str(" class=\"task-list-item\"");
            }
            self.out.push('>');
            self.pending_task = item.task;
            if !matches!(item.blocks.first(), Some(Block::Paragraph(_)))
                && let Some(checked) = self.pending_task.take()
            {
                self.task_marker(checked);
            }
            self.blocks(&item.blocks, list.tight);
            self.pending_task = None;
            self.out.push_str("</li>\n");
            self.ensure_line();
        }
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push_str(">\n");
    }

    fn code(&mut self, info: Option<&str>, literal: &str, line: usize) {
        self.ensure_line();
        let language = info.and_then(|info| info.split_whitespace().next());
        if language.is_some_and(mermaid::is_mermaid_language) {
            let number = self.result.diagrams.len() + 1;
            if let Some(diagnostic) = mermaid::limit_diagnostic(number, literal, line, self.options)
            {
                self.result.diagnostics.push(diagnostic);
            }
            self.result.diagrams.push(Diagram {
                id: format!("mermaid-{number}"),
                diagram_type: "mermaid".to_owned(),
                source_code: mermaid::diagram_source(literal),
                line_number: line,
            });
        }
        self.out.push_str("<pre><code");
        if let Some(language) = language {
            self.out.push_str(" class=\"language-");
            escape_html(language, &mut self.out);
            self.out.push('"');
        }
        self.out.push('>');
        escape_html(literal, &mut self.out);
        if !literal.is_empty() && !literal.ends_with('\n') {
            self.out.push('\n');
        }
        self.out.push_str("</code></pre>\n");
    }

    fn table(&mut self, table: &Table) {
        self.ensure_line();
        self.out.push_str("<table>\n");
        if table
            .widths
            .iter()
            .any(|width| *width != 0.0 && *width != 1.0)
        {
            for width in &table.widths {
                self.out.push_str("<col style=\"width:");
                self.out.push_str(&format_width(*width));
                self.out.push_str("%\" />\n");
            }
        }
        let mut has_body = false;
        let mut has_header = false;
        let mut header_open = false;
        for row in &table.rows {
            if self.over_limit() {
                break;
            }
            if row.header {
                if !has_header {
                    self.out.push_str("<thead>\n");
                    header_open = true;
                }
                has_header = true;
            } else if !has_body {
                if header_open {
                    self.out.push_str("</thead>\n");
                    header_open = false;
                }
                self.out.push_str("<tbody>\n");
                has_body = true;
            }
            self.out.push_str("<tr>\n");
            for (index, cell) in row.cells.iter().enumerate() {
                self.out.push_str(if row.header { "<th" } else { "<td" });
                if !table.alignments.is_empty() {
                    let column = index.min(table.alignments.len() - 1);
                    match table.alignments[column] {
                        Alignment::Left => self.out.push_str(" style=\"text-align: left;\""),
                        Alignment::Center => self.out.push_str(" style=\"text-align: center;\""),
                        Alignment::Right => self.out.push_str(" style=\"text-align: right;\""),
                        Alignment::None => {}
                    }
                }
                self.out.push('>');
                self.blocks(cell, cell.len() == 1);
                self.out
                    .push_str(if row.header { "</th>\n" } else { "</td>\n" });
            }
            self.out.push_str("</tr>\n");
        }
        if has_body {
            self.out.push_str("</tbody>\n");
        } else if header_open {
            self.out.push_str("</thead>\n");
        }
        self.out.push_str("</table>\n");
    }

    fn footnote_group(&mut self) {
        let plan = self.footnotes;
        if plan.ordered.is_empty() {
            return;
        }
        self.ensure_line();
        self.out
            .push_str("<div class=\"footnotes\">\n<hr />\n<ol>\n");
        for (index, definition) in plan.ordered.iter().enumerate() {
            if self.over_limit() {
                return;
            }
            let order = index + 1;
            self.out.push_str(&format!("<li id=\"fn:{order}\">\n"));
            let mut backlinks = String::new();
            for link in 1..=plan.reference_counts[index] {
                if self.out.len() + backlinks.len() > self.limit {
                    // Leave `out` over the limit so `render` reports it.
                    self.out.push_str(&backlinks);
                    return;
                }
                let link_index = plan.link_base[index] + link;
                backlinks.push_str(&format!(
                    "<a href=\"#fnref:{link_index}\" class=\"footnote-back-ref\">&#8617;</a>"
                ));
            }
            match definition.blocks.split_last() {
                Some((Block::Paragraph(last), rest)) => {
                    self.blocks(rest, false);
                    self.paragraph(last, rest.is_empty(), false, &backlinks);
                }
                _ => {
                    self.blocks(&definition.blocks, false);
                    self.ensure_line();
                    self.out.push_str("<p>");
                    self.out.push_str(&backlinks);
                    self.out.push_str("</p>\n");
                }
            }
            self.out.push_str("</li>\n");
        }
        self.out.push_str("</ol>\n</div>\n");
    }

    /// Markdig adapter metadata: links, images, and inline HTML anchors in
    /// document order, attributed to the containing leaf block's line.
    fn collect(&mut self, inlines: &[Inline], line: usize) {
        for inline in inlines {
            match inline {
                Inline::Link {
                    kind: LinkKind::Standard,
                    url,
                    ..
                } if !url.is_empty() => self.result.links.push(url.clone()),
                Inline::Image { url, .. } if !url.is_empty() => {
                    self.result.images.push(url.clone());
                }
                Inline::Html(tag) => extract_anchors(tag, line, &mut self.result.custom_anchors),
                _ => {}
            }
            match inline {
                Inline::Emphasis(children)
                | Inline::Strong(children)
                | Inline::Strikethrough(children)
                | Inline::Mark(children)
                | Inline::Subscript(children)
                | Inline::Superscript(children)
                | Inline::Inserted(children)
                | Inline::Link { children, .. }
                | Inline::Image { children, .. } => self.collect(children, line),
                _ => {}
            }
        }
    }

    fn inlines(&mut self, inlines: &[Inline], plain: bool) {
        for inline in inlines {
            if self.over_limit() {
                return;
            }
            self.inline(inline, plain);
        }
    }

    fn wrap(&mut self, tag: &str, children: &[Inline], plain: bool) {
        if plain {
            self.inlines(children, true);
            return;
        }
        self.out.push('<');
        self.out.push_str(tag);
        self.out.push('>');
        self.inlines(children, false);
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push('>');
    }

    fn inline(&mut self, inline: &Inline, plain: bool) {
        match inline {
            Inline::Text(text) | Inline::Escaped(text) => escape_html(text, &mut self.out),
            Inline::Code(code) => {
                if plain {
                    escape_html(code, &mut self.out);
                } else {
                    self.out.push_str("<code>");
                    escape_html(code, &mut self.out);
                    self.out.push_str("</code>");
                }
            }
            Inline::SoftBreak => self.out.push('\n'),
            Inline::HardBreak => {
                if !plain {
                    self.out.push_str("<br />");
                }
                self.out.push('\n');
            }
            Inline::Html(html) => {
                if !plain {
                    self.out.push_str(html);
                }
            }
            Inline::Emphasis(children) => self.wrap("em", children, plain),
            Inline::Strong(children) => self.wrap("strong", children, plain),
            Inline::Strikethrough(children) => self.wrap("del", children, plain),
            Inline::Mark(children) => self.wrap("mark", children, plain),
            Inline::Subscript(children) => self.wrap("sub", children, plain),
            Inline::Superscript(children) => self.wrap("sup", children, plain),
            Inline::Inserted(children) => self.wrap("ins", children, plain),
            Inline::Link {
                url,
                title,
                children,
                ..
            } => {
                if plain {
                    self.inlines(children, true);
                    return;
                }
                self.out.push_str("<a href=\"");
                escape_url(url, &mut self.out);
                self.out.push('"');
                self.title(title);
                self.out.push('>');
                self.inlines(children, false);
                self.out.push_str("</a>");
            }
            Inline::Image {
                url,
                title,
                children,
            } => {
                if plain {
                    self.inlines(children, true);
                    return;
                }
                self.out.push_str("<img src=\"");
                escape_url(url, &mut self.out);
                self.out.push_str("\" alt=\"");
                self.inlines(children, true);
                self.out.push('"');
                self.title(title);
                self.out.push_str(" />");
            }
            Inline::FootnoteReference(label) => {
                if plain {
                    return;
                }
                let Some(&index) = self.footnotes.order.get(&normalize_label(label)) else {
                    return;
                };
                self.references_seen[index] += 1;
                let link_index = self.footnotes.link_base[index] + self.references_seen[index];
                let order = index + 1;
                self.out.push_str(&format!(
                    "<a id=\"fnref:{link_index}\" href=\"#fn:{order}\" class=\"footnote-ref\"><sup>{order}</sup></a>"
                ));
            }
            Inline::Math(math) => {
                if plain {
                    escape_html(math, &mut self.out);
                } else {
                    self.out.push_str("<span class=\"math\">\\(");
                    escape_html(math, &mut self.out);
                    self.out.push_str("\\)</span>");
                }
            }
        }
    }

    fn title(&mut self, title: &str) {
        if !title.is_empty() {
            self.out.push_str(" title=\"");
            escape_html(title, &mut self.out);
            self.out.push('"');
        }
    }
}

/// `MarkdigParserAdapter.ExtractPlainText`: literals and code spans, recursing
/// into container inlines. Markdig autolinks are leaves and contribute nothing.
fn heading_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Escaped(text) | Inline::Code(text) => out.push_str(text),
            Inline::Link {
                kind: LinkKind::Autolink,
                ..
            } => {}
            Inline::Emphasis(children)
            | Inline::Strong(children)
            | Inline::Strikethrough(children)
            | Inline::Mark(children)
            | Inline::Subscript(children)
            | Inline::Superscript(children)
            | Inline::Inserted(children)
            | Inline::Link { children, .. }
            | Inline::Image { children, .. } => heading_text(children, out),
            _ => {}
        }
    }
}

/// Markdig `HtmlHelper.EscapeHtml`: `<`, `>`, `&`, and `"`.
pub(crate) fn escape_html(text: &str, out: &mut String) {
    let mut last = 0;
    for (index, byte) in text.bytes().enumerate() {
        let replacement = match byte {
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'&' => "&amp;",
            b'"' => "&quot;",
            _ => continue,
        };
        out.push_str(&text[last..index]);
        out.push_str(replacement);
        last = index + 1;
    }
    out.push_str(&text[last..]);
}

/// Markdig `HtmlRenderer.WriteEscapeUrl`: percent-encode ASCII controls, space,
/// DEL, ``"'<>[\]^`{|}~`` and all non-ASCII UTF-8 bytes; HTML-escape `&`.
pub(crate) fn escape_url(url: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut last = 0;
    for (index, byte) in url.bytes().enumerate() {
        let encode = byte <= 32 || byte >= 127 || b"\"'<>[\\]^`{|}~".contains(&byte);
        if !encode && byte != b'&' {
            continue;
        }
        // Every non-ASCII byte is encoded, so unencoded runs are ASCII-bounded.
        if index > last {
            out.push_str(&url[last..index]);
        }
        if byte == b'&' {
            out.push_str("&amp;");
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        last = index + 1;
    }
    out.push_str(&url[last..]);
}

/// Markdig: `Math.Round(width * 100) / 100` formatted with `{0:0.##}`.
fn format_width(width: f32) -> String {
    let rounded = f64::from(width * 100.0).round_ties_even() / 100.0;
    let text = format!("{rounded:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_escaping_matches_markdig_table() {
        let mut out = String::new();
        escape_url("javascript:alert('xss') a&b é%20[x]", &mut out);
        assert_eq!(
            out,
            "javascript:alert(%27xss%27)%20a&amp;b%20%C3%A9%20%5Bx%5D"
        );
    }

    #[test]
    fn html_escaping_keeps_single_quotes() {
        let mut out = String::new();
        escape_html("<a href=\"x\">'&'</a>", &mut out);
        assert_eq!(out, "&lt;a href=&quot;x&quot;&gt;'&amp;'&lt;/a&gt;");
    }

    #[test]
    fn widths_use_markdig_rounding_and_format() {
        assert_eq!(format_width(50.0), "50");
        assert_eq!(format_width(100.0 / 3.0), "33.33");
        assert_eq!(format_width(12.5), "12.5");
    }

    #[test]
    fn footnote_links_are_numbered_globally_in_footnote_order() {
        let reference = |label: &str| Inline::FootnoteReference(label.to_owned());
        let paragraph = |inlines| {
            Block::Paragraph(Paragraph {
                line: 0,
                end_line: 0,
                inlines,
            })
        };
        let definition = |label: &str, text: &str| FootnoteDefinition {
            label: label.to_owned(),
            blocks: vec![paragraph(vec![Inline::Text(text.to_owned())])],
        };
        let document = Document {
            blocks: vec![paragraph(vec![
                reference("b"),
                reference("a"),
                reference("B"),
            ])],
            footnotes: vec![definition("a", "A"), definition("b", "B")],
            diagnostics: Vec::new(),
        };
        let html = render(&document, &MarkdownOptions::default(), 0)
            .unwrap()
            .rendered_body_html;
        assert_eq!(
            html,
            "<p><a id=\"fnref:1\" href=\"#fn:1\" class=\"footnote-ref\"><sup>1</sup></a>\
<a id=\"fnref:3\" href=\"#fn:2\" class=\"footnote-ref\"><sup>2</sup></a>\
<a id=\"fnref:2\" href=\"#fn:1\" class=\"footnote-ref\"><sup>1</sup></a></p>\n\
<div class=\"footnotes\">\n<hr />\n<ol>\n<li id=\"fn:1\">\n\
<p>B<a href=\"#fnref:1\" class=\"footnote-back-ref\">&#8617;</a><a href=\"#fnref:2\" class=\"footnote-back-ref\">&#8617;</a></p>\n\
</li>\n<li id=\"fn:2\">\n<p>A<a href=\"#fnref:3\" class=\"footnote-back-ref\">&#8617;</a></p>\n</li>\n</ol>\n</div>\n"
        );
    }
}
