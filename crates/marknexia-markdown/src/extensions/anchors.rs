//! Custom anchor extraction equivalent to the .NET adapter's
//! `<a\s+[^>]*(?:name|id)\s*=\s*["']([^"']+)["'][^>]*>` (case-insensitive,
//! global) over HTML block text and inline HTML tags.

use crate::Anchor;

/// Linear-time emulation. Two facts keep it linear while preserving the
/// regex's leftmost, non-overlapping matches:
///
/// * Every match ends at a `>`, so nothing can match once no `>` remains.
/// * All `<a` starts before the same first `>` share that `>` as the bound for
///   the attribute key, and a key's success does not depend on the start, so
///   if the earliest such start fails, every later start before that `>` fails.
pub(crate) fn extract_anchors(html: &str, line_number: usize, anchors: &mut Vec<Anchor>) {
    let Some(last_gt) = html.rfind('>') else {
        return;
    };
    let mut search = 0;
    let mut next_gt = 0;
    while let Some(start) = find_anchor_start(html, search) {
        // `<a` followed by at least one whitespace character.
        let Some(body_start) = after_whitespace(html, start + 2) else {
            search = start + 1;
            continue;
        };
        // `[^>]*` cannot cross `>`, so the attribute key starts before the first `>`.
        if next_gt < body_start {
            match html[body_start..].find('>') {
                Some(offset) => next_gt = body_start + offset,
                None => return,
            }
        }
        let prefix_end = next_gt;
        match last_attribute_value(html, body_start, prefix_end, last_gt) {
            Some((value, end)) => {
                anchors.push(Anchor {
                    id: value.to_owned(),
                    name: value.to_owned(),
                    is_heading_anchor: false,
                    line_number,
                });
                search = end;
            }
            None => search = prefix_end,
        }
    }
}

/// Next `<a` or `<A` at or after `from`, found with memchr-backed `find`.
fn find_anchor_start(html: &str, mut from: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    loop {
        let start = from + html.get(from..)?.find('<')?;
        if bytes
            .get(start + 1)
            .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'a'))
        {
            return Some(start);
        }
        from = start + 1;
    }
}

/// Returns the offset just after the first whitespace character at `index`.
fn after_whitespace(text: &str, index: usize) -> Option<usize> {
    let ch = text.get(index..)?.chars().next()?;
    ch.is_whitespace().then(|| index + ch.len_utf8())
}

/// The regex's greedy `[^>]*` prefix selects the right-most `name`/`id` key
/// that yields a complete match. Returns the value and the match end.
fn last_attribute_value(
    html: &str,
    body_start: usize,
    prefix_end: usize,
    last_gt: usize,
) -> Option<(&str, usize)> {
    let bytes = html.as_bytes();
    let key_at = |position: usize, key: &[u8]| {
        bytes
            .get(position..position + key.len())
            .is_some_and(|window| window.eq_ignore_ascii_case(key))
    };
    (body_start..prefix_end).rev().find_map(|position| {
        let key_len = if key_at(position, b"name") {
            4
        } else if key_at(position, b"id") {
            2
        } else {
            return None;
        };
        attribute_value(html, position + key_len, prefix_end, last_gt)
    })
}

/// `\s*=\s*["']([^"']+)["'][^>]*>` from `index`; returns the value and match end.
fn attribute_value(
    html: &str,
    mut index: usize,
    prefix_end: usize,
    last_gt: usize,
) -> Option<(&str, usize)> {
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
    // Closing quote, then `[^>]*>`: the first `>` after the closing quote.
    // Opening quotes precede `prefix_end`, so only a value that itself spans
    // `>` needs a search, and none can succeed past the last `>`.
    let close = if value_end < prefix_end {
        prefix_end
    } else if value_end < last_gt {
        html[value_end + 1..].find('>')? + value_end + 1
    } else {
        return None;
    };
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
