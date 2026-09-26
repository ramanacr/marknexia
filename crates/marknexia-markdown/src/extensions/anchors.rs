//! Custom anchor extraction equivalent to the .NET adapter's
//! `<a\s+[^>]*(?:name|id)\s*=\s*["']([^"']+)["'][^>]*>` (case-insensitive,
//! global) over HTML block text and inline HTML tags.

use crate::Anchor;

pub(crate) fn extract_anchors(html: &str, line_number: usize, anchors: &mut Vec<Anchor>) {
    if html.trim().is_empty() {
        return;
    }
    let bytes = html.as_bytes();
    let mut search = 0;
    while let Some(relative) = find_anchor_start(&bytes[search..]) {
        let start = search + relative;
        // `<a` followed by at least one whitespace character.
        let Some(body_start) = after_whitespace(html, start + 2) else {
            search = start + 1;
            continue;
        };
        // `[^>]*` cannot cross `>`, so the attribute key starts before the first `>`.
        let prefix_end = html[body_start..]
            .find('>')
            .map_or(html.len(), |offset| body_start + offset);
        match last_attribute_value(html, body_start, prefix_end) {
            Some((value, end)) => {
                anchors.push(Anchor {
                    id: value.to_owned(),
                    name: value.to_owned(),
                    is_heading_anchor: false,
                    line_number,
                });
                search = end;
            }
            None => search = start + 1,
        }
    }
}

fn find_anchor_start(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|pair| pair[0] == b'<' && pair[1].eq_ignore_ascii_case(&b'a'))
}

/// Returns the offset just after the first whitespace character at `index`.
fn after_whitespace(text: &str, index: usize) -> Option<usize> {
    let ch = text.get(index..)?.chars().next()?;
    ch.is_whitespace().then(|| index + ch.len_utf8())
}

/// The regex's greedy `[^>]*` prefix selects the right-most `name`/`id` key
/// that yields a complete match. Returns the value and the match end.
fn last_attribute_value(html: &str, body_start: usize, prefix_end: usize) -> Option<(&str, usize)> {
    let lower = html[body_start..prefix_end].to_ascii_lowercase();
    let mut positions: Vec<usize> = lower.char_indices().map(|(index, _)| index).collect();
    positions.reverse();
    positions.into_iter().find_map(|position| {
        let rest = &lower[position..];
        let key_len = if rest.starts_with("name") {
            4
        } else if rest.starts_with("id") {
            2
        } else {
            return None;
        };
        attribute_value(html, body_start + position + key_len)
    })
}

fn attribute_value(html: &str, mut index: usize) -> Option<(&str, usize)> {
    let skip_whitespace = |mut at: usize| {
        while let Some(ch) = html[at..].chars().next().filter(|ch| ch.is_whitespace()) {
            at += ch.len_utf8();
        }
        at
    };
    index = skip_whitespace(index);
    if !html[index..].starts_with('=') {
        return None;
    }
    index = skip_whitespace(index + 1);
    if !html[index..].starts_with(['"', '\'']) {
        return None;
    }
    let value_start = index + 1;
    let value_end = html[value_start..]
        .find(['"', '\''])
        .map(|offset| value_start + offset)?;
    if value_end == value_start {
        return None;
    }
    // Closing quote, then `[^>]*>`.
    let close = html[value_end + 1..].find('>')? + value_end + 1;
    Some((&html[value_start..value_end], close + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(html: &str) -> Vec<String> {
        let mut anchors = Vec::new();
        extract_anchors(html, 3, &mut anchors);
        assert!(anchors.iter().all(|anchor| anchor.line_number == 3
            && !anchor.is_heading_anchor
            && anchor.id == anchor.name));
        anchors.into_iter().map(|anchor| anchor.id).collect()
    }

    #[test]
    fn matches_dotnet_regex_semantics() {
        assert_eq!(ids("<a id=\"legacy-anchor\"></a>"), ["legacy-anchor"]);
        assert_eq!(ids("<A NAME='custom anchor'>"), ["custom anchor"]);
        // Greedy prefix: the right-most attribute wins, including `data-id`.
        assert_eq!(ids("<a id=\"first\" data-id=\"second\">"), ["second"]);
        // Mixed quotes terminate at the first quote of either kind.
        assert_eq!(ids("<a id=\"mixed'>"), ["mixed"]);
        assert_eq!(ids("<a id=\"\">"), Vec::<String>::new());
        assert_eq!(ids("<abbr id=\"x\"><a\nid=\"y\"><a id=z>"), ["y"]);
        assert_eq!(ids("<a id=\"one\"></a> <a name=\"two\">"), ["one", "two"]);
        assert_eq!(ids("<a id=\"unterminated\""), Vec::<String>::new());
        // The captured value may contain `>`; the key may not follow one.
        assert_eq!(ids("<a id=\"a>b\">"), ["a>b"]);
        assert_eq!(ids("<a > id=\"late\">"), Vec::<String>::new());
    }
}
