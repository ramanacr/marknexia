use crate::{
    Heading, MarkdownEngine, MarkdownOptions, ParsedDocument,
    compatibility::{ComrakHeadingIds, extract_anchors, push_mermaid},
};
use comrak::{Arena, Options, nodes::NodeValue};
use marknexia_core::{
    contracts::Diagnostic,
    slug::{SlugSet, generate_heading_slug},
};

/// Exploratory candidate. Output is content-unsafe/unbounded; never pass it to
/// WebView without sanitization and output limits.
pub struct ComrakAdapter;

impl MarkdownEngine for ComrakAdapter {
    fn parse(&self, source: &str, _: &MarkdownOptions) -> Result<ParsedDocument, Vec<Diagnostic>> {
        let mut options = Options::default();
        options.extension.table = true;
        options.extension.tasklist = true;
        options.extension.strikethrough = true;
        options.extension.footnotes = true;
        options.render.r#unsafe = true;
        let arena = Arena::new();
        let root = comrak::parse_document(&arena, source, &options);
        let mut result = ParsedDocument::default();
        let mut slugs = SlugSet::default();
        for node in root.descendants() {
            let data = node.data.borrow();
            match &data.value {
                NodeValue::Heading(heading) => {
                    let mut text = String::new();
                    for child in node.descendants().skip(1) {
                        match &child.data.borrow().value {
                            NodeValue::Text(value) => text.push_str(value),
                            NodeValue::Code(value) => text.push_str(&value.literal),
                            _ => {}
                        }
                    }
                    let text = text.trim().to_owned();
                    result.headings.push(Heading {
                        slug_id: generate_heading_slug(&text, &mut slugs),
                        text,
                        level: heading.level,
                        line_number: data.sourcepos.start.line.saturating_sub(1),
                    });
                }
                NodeValue::Link(link) => result.links.push(link.url.clone()),
                NodeValue::Image(image) => result.images.push(image.url.clone()),
                NodeValue::HtmlBlock(html) => extract_anchors(
                    &html.literal,
                    data.sourcepos.start.line.saturating_sub(1),
                    &mut result.custom_anchors,
                ),
                NodeValue::HtmlInline(html) => extract_anchors(
                    html,
                    data.sourcepos.start.line.saturating_sub(1),
                    &mut result.custom_anchors,
                ),
                NodeValue::CodeBlock(code) => push_mermaid(
                    &code.info,
                    &code.literal,
                    data.sourcepos.start.line.saturating_sub(1),
                    &mut result.diagrams,
                ),
                _ => {}
            }
        }
        let mut html = String::new();
        let heading_ids = ComrakHeadingIds {
            headings: &result.headings,
            next: std::sync::Mutex::new(0),
        };
        let mut plugins = comrak::options::Plugins::default();
        plugins.render.heading_adapter = Some(&heading_ids);
        comrak::format_html_with_plugins(root, &options, &mut html, &plugins)
            .map_err(|error| vec![Diagnostic::warning(error.to_string())])?;
        result.rendered_body_html = html;
        Ok(result)
    }
}
