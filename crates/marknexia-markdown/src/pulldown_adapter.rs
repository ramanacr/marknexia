use crate::{
    Heading, MarkdownEngine, MarkdownOptions, ParsedDocument,
    compatibility::{extract_anchors, heading_id, push_mermaid},
};
use marknexia_core::{
    contracts::Diagnostic,
    slug::{SlugSet, generate_heading_slug},
};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};

/// Exploratory candidate. Output is content-unsafe/unbounded; never pass it to
/// WebView without sanitization and output limits.
pub struct PulldownAdapter;

impl MarkdownEngine for PulldownAdapter {
    fn parse(&self, source: &str, _: &MarkdownOptions) -> Result<ParsedDocument, Vec<Diagnostic>> {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_STRIKETHROUGH;
        let mut result = ParsedDocument::default();
        let mut slugs = SlugSet::default();
        let mut heading: Option<(u8, usize, String)> = None;
        let mut mermaid: Option<(usize, String)> = None;
        for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
            match event {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                    if info.trim().eq_ignore_ascii_case("mermaid") =>
                {
                    let line = source[..range.start]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count();
                    mermaid = Some((line, String::new()));
                }
                Event::End(TagEnd::CodeBlock) => {
                    if let Some((line, code)) = mermaid.take() {
                        push_mermaid("mermaid", &code, line, &mut result.diagrams);
                    }
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    let line = source[..range.start]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count();
                    heading = Some((level as u8, line, String::new()));
                }
                Event::End(TagEnd::Heading(_)) => {
                    if let Some((level, line_number, text)) = heading.take() {
                        let text = text.trim().to_owned();
                        result.headings.push(Heading {
                            slug_id: generate_heading_slug(&text, &mut slugs),
                            text,
                            level,
                            line_number,
                        });
                    }
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    result.links.push(dest_url.into_string())
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    result.images.push(dest_url.into_string())
                }
                Event::Html(value) | Event::InlineHtml(value) => {
                    let line = source[..range.start]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count();
                    extract_anchors(&value, line, &mut result.custom_anchors);
                }
                Event::Text(value) | Event::Code(value) => {
                    if let Some((_, code)) = &mut mermaid {
                        code.push_str(&value);
                    }
                    if let Some((_, _, text)) = &mut heading {
                        text.push_str(&value);
                    }
                }
                _ => {}
            }
        }
        let mut next_heading = 0;
        let events = Parser::new_ext(source, options).map(|event| {
            if let Event::Start(Tag::Heading {
                level,
                id: _,
                classes,
                attrs,
            }) = event
            {
                let id = heading_id(&result.headings, next_heading).map(str::to_owned);
                next_heading += 1;
                Event::Start(Tag::Heading {
                    level,
                    id: id.map(Into::into),
                    classes,
                    attrs,
                })
            } else {
                event
            }
        });
        html::push_html(&mut result.rendered_body_html, events);
        Ok(result)
    }
}
