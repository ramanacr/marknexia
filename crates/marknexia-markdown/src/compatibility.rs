//! Marknexia-owned exploratory compatibility helpers shared by parser candidates.
//! This is metadata/rendering compatibility only; HTML remains content-unsafe and unbounded.

use crate::{Anchor, Diagram, Heading};

pub(crate) fn push_mermaid(
    info: &str,
    source: &str,
    line_number: usize,
    diagrams: &mut Vec<Diagram>,
) {
    if !info.trim().eq_ignore_ascii_case("mermaid") {
        return;
    }
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    diagrams.push(Diagram {
        id: format!("mermaid-{}", diagrams.len() + 1),
        diagram_type: "mermaid".to_owned(),
        source_code: normalized.trim_end_matches('\n').replace('\n', "\r\n"),
        line_number,
    });
}

pub(crate) fn extract_anchors(html: &str, line_number: usize, anchors: &mut Vec<Anchor>) {
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if !rest
            .get(..1)
            .is_some_and(|letter| letter.eq_ignore_ascii_case("a"))
            || !rest.as_bytes().get(1).is_some_and(u8::is_ascii_whitespace)
        {
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        let mut cursor = 1;
        let bytes = tag.as_bytes();
        while cursor < bytes.len() {
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            let key_start = cursor;
            while cursor < bytes.len()
                && !bytes[cursor].is_ascii_whitespace()
                && bytes[cursor] != b'='
            {
                cursor += 1;
            }
            let key = &tag[key_start..cursor];
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if bytes.get(cursor) != Some(&b'=') {
                if cursor == key_start {
                    break;
                }
                continue;
            }
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            let Some(&quote @ (b'\'' | b'"')) = bytes.get(cursor) else {
                continue;
            };
            cursor += 1;
            let value_start = cursor;
            while cursor < bytes.len() && bytes[cursor] != quote {
                cursor += 1;
            }
            if (key.eq_ignore_ascii_case("id") || key.eq_ignore_ascii_case("name"))
                && cursor > value_start
            {
                let value = &tag[value_start..cursor];
                anchors.push(Anchor {
                    id: value.to_owned(),
                    name: value.to_owned(),
                    is_heading_anchor: false,
                    line_number,
                });
                break;
            }
            cursor += usize::from(cursor < bytes.len());
        }
        rest = &rest[end + 1..];
    }
}

pub(crate) fn heading_id(headings: &[Heading], index: usize) -> Option<&str> {
    headings.get(index).map(|heading| heading.slug_id.as_str())
}

#[cfg(feature = "candidate-comrak")]
pub(crate) struct ComrakHeadingIds<'a> {
    pub headings: &'a [Heading],
    pub next: std::sync::Mutex<usize>,
}

#[cfg(feature = "candidate-comrak")]
impl comrak::adapters::HeadingAdapter for ComrakHeadingIds<'_> {
    fn enter(
        &self,
        output: &mut dyn std::fmt::Write,
        heading: &comrak::adapters::HeadingMeta,
        _: Option<comrak::nodes::Sourcepos>,
    ) -> std::fmt::Result {
        let mut next = self.next.lock().map_err(|_| std::fmt::Error)?;
        let id = heading_id(self.headings, *next).ok_or(std::fmt::Error)?;
        *next += 1;
        write!(output, "<h{} id=\"{}\">", heading.level, id)
    }

    fn exit(
        &self,
        output: &mut dyn std::fmt::Write,
        heading: &comrak::adapters::HeadingMeta,
    ) -> std::fmt::Result {
        writeln!(output, "</h{}>", heading.level)
    }
}
