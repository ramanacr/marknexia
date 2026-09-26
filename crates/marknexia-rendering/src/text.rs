//! .NET text primitives the renderer's string passes depend on:
//! `WebUtility.HtmlEncode`/`HtmlDecode`, `Uri.EscapeDataString`, and the
//! regex character classes `\s` and `\w`.
//!
//! Everything these helpers produce is fed to the parser-backed sanitizer, so
//! encoders only need to be DOM-equivalent to .NET (the sanitizer
//! re-serializes text); the decoder must be exact because decoded text
//! becomes copy-button metadata and highlighted code.

use markup5ever::data::NAMED_ENTITIES;

/// .NET regex `\s` (`[\f\n\r\t\v\x85\p{Z}]`), which is the Unicode
/// `White_Space` set that `char::is_whitespace` implements.
pub(crate) fn is_space(character: char) -> bool {
    character.is_whitespace()
}

/// .NET regex `\w`. Exact for ASCII; non-ASCII uses Rust's
/// alphabetic/numeric properties (see the rendering decision, REND-6).
pub(crate) fn is_word(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

/// Entity-encode text for an HTML text or double-quoted attribute context.
pub(crate) fn html_encode(text: &str, out: &mut String) {
    let mut last = 0;
    for (index, byte) in text.bytes().enumerate() {
        let replacement = match byte {
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'&' => "&amp;",
            b'"' => "&quot;",
            b'\'' => "&#39;",
            _ => continue,
        };
        out.push_str(&text[last..index]);
        out.push_str(replacement);
        last = index + 1;
    }
    out.push_str(&text[last..]);
}

pub(crate) fn html_encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    html_encode(text, &mut out);
    out
}

/// `System.Net.WebUtility.HtmlDecode`: an entity runs from `&` to the next
/// `;` (an intervening `&` ends it undecoded). Numeric references are decimal
/// or `x`/`X` hex; named references use the HTML5 table (a superset of the
/// .NET HTML 4 table, see REND-6). Unknown entities stay verbatim.
pub(crate) fn html_decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let Some(end) = after.find([';', '&']) else {
            out.push_str(&rest[amp..]);
            return out;
        };
        if after.as_bytes()[end] == b'&' {
            out.push('&');
            rest = after;
            continue;
        }
        let entity = &after[..end];
        if decode_entity(entity, &mut out) {
            rest = &after[end + 1..];
        } else {
            out.push('&');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(entity: &str, out: &mut String) -> bool {
    if let Some(number) = entity.strip_prefix('#') {
        let parsed = match number.strip_prefix(['x', 'X']) {
            Some(hex) if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
                u32::from_str_radix(hex, 16).ok()
            }
            Some(_) => None,
            None => {
                // `uint.TryParse(..., NumberStyles.Integer)`: surrounding
                // ASCII white space and a leading `+` are accepted.
                let digits = number.trim_matches(|c: char| matches!(c, '\t'..='\r' | ' '));
                let digits = digits.strip_prefix('+').unwrap_or(digits);
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                    digits.parse::<u32>().ok()
                } else {
                    None
                }
            }
        };
        return match parsed {
            Some(value) if value <= 0x10_FFFF => {
                // .NET emits a lone UTF-16 surrogate here; a Rust string
                // cannot hold one, so it becomes U+FFFD.
                out.push(char::from_u32(value).unwrap_or('\u{FFFD}'));
                true
            }
            _ => false,
        };
    }
    if entity.is_empty() || !entity.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    let key = format!("{entity};");
    match NAMED_ENTITIES.get(key.as_str()) {
        Some(&(first, second)) => {
            for code in [first, second] {
                if code != 0 {
                    out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                }
            }
            true
        }
        None => false,
    }
}

/// `Uri.EscapeDataString`: every UTF-8 byte outside the RFC 3986 unreserved
/// set becomes `%XX` with upper-case hex.
pub(crate) fn escape_data_string(text: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
}

/// ASCII case-insensitive `starts_with` at a byte offset.
pub(crate) fn starts_with_ci(haystack: &str, at: usize, needle: &str) -> bool {
    haystack
        .as_bytes()
        .get(at..at + needle.len())
        .is_some_and(|bytes| bytes.eq_ignore_ascii_case(needle.as_bytes()))
}

/// ASCII case-insensitive search for an ASCII `needle` from byte `from`.
pub(crate) fn find_ci(haystack: &str, from: usize, needle: &str) -> Option<usize> {
    let bytes = haystack.as_bytes();
    let needle = needle.as_bytes();
    let first = needle[0].to_ascii_lowercase();
    let mut index = from;
    while index + needle.len() <= bytes.len() {
        if bytes[index].to_ascii_lowercase() == first
            && bytes[index..index + needle.len()].eq_ignore_ascii_case(needle)
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// Byte offset just past the run of `\s` starting at `from`.
pub(crate) fn skip_space(text: &str, from: usize) -> usize {
    text[from..]
        .char_indices()
        .find(|&(_, character)| !is_space(character))
        .map_or(text.len(), |(offset, _)| from + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_matches_webutility() {
        assert_eq!(
            html_decode("a &lt;b&gt; &amp;amp; &quot;"),
            "a <b> &amp; \""
        );
        assert_eq!(html_decode("&#65;&#x42;&#X43;&copy;"), "ABC\u{a9}");
        assert_eq!(html_decode("&unknown; &amp &lt&gt;"), "&unknown; &amp &lt>");
        assert_eq!(html_decode("&#;&#x;&#1114112;"), "&#;&#x;&#1114112;");
        assert_eq!(html_decode("no entities"), "no entities");
    }

    #[test]
    fn escape_matches_uri_escape_data_string() {
        let mut out = String::new();
        escape_data_string("var value = 1;\n-_.~é", &mut out);
        assert_eq!(out, "var%20value%20%3D%201%3B%0A-_.~%C3%A9");
    }

    #[test]
    fn case_insensitive_search() {
        assert_eq!(find_ci("xx<PRE><code", 0, "<pre><code"), Some(2));
        assert_eq!(find_ci("<pre", 1, "<pre"), None);
        assert!(starts_with_ci("CLASS=", 0, "class="));
        assert_eq!(skip_space("a \n\tb", 1), 4);
    }
}
