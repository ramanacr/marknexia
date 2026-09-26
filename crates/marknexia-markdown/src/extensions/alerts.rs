//! GitHub alerts, reproducing the .NET post-render transform
//! `<blockquote>\s*<p>\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\](?:\s*<br\s*/?>|\s*\r?\n)?(.*?)</p>\s*(.*?)</blockquote>`
//! (singleline, case-insensitive) so quirks such as nested quotes match exactly.

use std::borrow::Cow;

const KINDS: [&str; 5] = ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"];

/// Applies the alert transform, or returns `None` once the output would exceed
/// `limit` bytes.
pub(crate) fn transform_gfm_alerts(html: &str, limit: usize) -> Option<Cow<'_, str>> {
    if html.trim().is_empty() {
        return Some(Cow::Borrowed(html));
    }
    let mut out = String::new();
    let mut copied = 0;
    let mut search = 0;
    while let Some(start) = find_ci(html, search, "<blockquote>") {
        match match_alert(html, start) {
            Outcome::Match(end, replacement) => {
                out.push_str(&html[copied..start]);
                out.push_str(&replacement);
                if out.len() > limit {
                    return None;
                }
                copied = end;
                search = end;
            }
            Outcome::NoMatch => search = start + 1,
            Outcome::Stop => break,
        }
    }
    if copied == 0 {
        return Some(Cow::Borrowed(html));
    }
    out.push_str(&html[copied..]);
    (out.len() <= limit).then_some(Cow::Owned(out))
}

enum Outcome {
    Match(usize, String),
    /// This `<blockquote>` cannot start a match; later ones still might.
    NoMatch,
    /// No `</p>` or `</blockquote>` remains where one is required. Every later
    /// `<blockquote>` starts at or after this candidate's alert marker and
    /// would search from the same point or later, so none can match: stopping
    /// keeps the scan linear without changing output.
    Stop,
}

fn match_alert(html: &str, start: usize) -> Outcome {
    let Some((index, kind, title)) = alert_head(html, start) else {
        return Outcome::NoMatch;
    };
    let Some(paragraph_end) = find_ci(html, index, "</p>") else {
        return Outcome::Stop;
    };
    let first_line = html[index..paragraph_end].trim();
    let remainder_start = skip_whitespace(html, paragraph_end + "</p>".len());
    let Some(quote_end) = find_ci(html, remainder_start, "</blockquote>") else {
        return Outcome::Stop;
    };
    let remainder = html[remainder_start..quote_end].trim();

    let kind = kind.to_ascii_lowercase();
    let mut replacement = String::with_capacity(512 + first_line.len() + remainder.len());
    replacement.push_str("<div class=\"markdown-alert markdown-alert-");
    replacement.push_str(&kind);
    replacement.push_str("\"><div class=\"markdown-alert-title\">");
    replacement.push_str(icon(&kind));
    replacement.push_str("<span>");
    replacement.push_str(title);
    replacement.push_str("</span></div>");
    if !first_line.is_empty() {
        replacement.push_str("<p>");
        replacement.push_str(first_line);
        replacement.push_str("</p>");
    }
    replacement.push_str(remainder);
    replacement.push_str("</div>");
    Outcome::Match(quote_end + "</blockquote>".len(), replacement)
}

/// `<blockquote>\s*<p>\s*\[!KIND\]` plus the optional `\s*<br\s*/?>`. Returns
/// the offset where the first-line capture starts, the kind, and the title as
/// written.
fn alert_head(html: &str, start: usize) -> Option<(usize, &'static str, &str)> {
    let mut index = skip_whitespace(html, start + "<blockquote>".len());
    index = expect_ci(html, index, "<p>")?;
    index = skip_whitespace(html, index);
    index = expect_ci(html, index, "[!")?;
    let kind = KINDS
        .iter()
        .find(|kind| expect_ci(html, index, kind).is_some())?;
    let title = &html[index..index + kind.len()];
    index = expect_ci(html, index + kind.len(), "]")?;
    if let Some(after_break) = match_break(html, skip_whitespace(html, index)) {
        index = after_break;
    }
    Some((index, kind, title))
}

/// `<br\s*/?>`
fn match_break(html: &str, index: usize) -> Option<usize> {
    let mut index = expect_ci(html, index, "<br")?;
    index = skip_whitespace(html, index);
    if html[index..].starts_with('/') {
        index += 1;
    }
    expect_ci(html, index, ">")
}

fn skip_whitespace(html: &str, mut index: usize) -> usize {
    while let Some(ch) = html[index..].chars().next().filter(|ch| ch.is_whitespace()) {
        index += ch.len_utf8();
    }
    index
}

fn expect_ci(html: &str, index: usize, literal: &str) -> Option<usize> {
    let end = index + literal.len();
    html.get(index..end)
        .filter(|text| text.eq_ignore_ascii_case(literal))
        .map(|_| end)
}

/// Case-insensitive search for a `<`-prefixed ASCII literal, skipping between
/// `<` bytes with the standard library's memchr-backed `find`.
fn find_ci(html: &str, mut from: usize, literal: &str) -> Option<usize> {
    debug_assert!(literal.starts_with('<'));
    loop {
        let start = from + html.get(from..)?.find('<')?;
        if expect_ci(html, start, literal).is_some() {
            return Some(start);
        }
        from = start + 1;
    }
}

fn icon(kind: &str) -> &'static str {
    match kind {
        "tip" => {
            r#"<svg class="octicon octicon-light-bulb" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M8 1.5c-2.363 0-4 1.69-4 3.75 0 .76.24 1.487.653 2.083.47.677.847 1.549.847 2.417h5c0-.868.377-1.74.847-2.417.413-.596.653-1.323.653-2.083 0-2.06-1.637-3.75-4-3.75Z"></path></svg>"#
        }
        "important" => {
            r#"<svg class="octicon octicon-report" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M0 1.75C0 .784.784 0 1.75 0h12.5C15.216 0 16 .784 16 1.75v9.5A1.75 1.75 0 0 1 14.25 13H9.06l-2.573 2.573A1.458 1.458 0 0 1 4 14.543V13H1.75A1.75 1.75 0 0 1 0 11.25Z"></path></svg>"#
        }
        "warning" => {
            r#"<svg class="octicon octicon-alert" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575Zm1.763.707a.25.25 0 0 0-.44 0L1.698 13.132a.25.25 0 0 0 .22.368h12.164a.25.25 0 0 0 .22-.368Zm.53 3.996v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z"></path></svg>"#
        }
        "caution" => {
            r#"<svg class="octicon octicon-stop" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M4.47.047A1.75 1.75 0 0 1 5.71 0h4.58c.464 0 .909.184 1.237.513l4.96 4.96c.329.328.513.773.513 1.237v4.58c0 .464-.184.909-.513 1.237l-4.96 4.96a1.75 1.75 0 0 1-1.237.513H5.71a1.75 1.75 0 0 1-1.237-.513l-4.96-4.96A1.75 1.75 0 0 1 0 11.29V6.71c0-.464.184-.909.513-1.237l4.96-4.96Z"></path></svg>"#
        }
        _ => {
            r#"<svg class="octicon octicon-info" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.5 7.75A.75.75 0 0 1 7.25 7h1a.75.75 0 0 1 .75.75v2.75h.25a.75.75 0 0 1 0 1.5h-2a.75.75 0 0 1 0-1.5h.25v-2h-.25a.75.75 0 0 1-.75-.75ZM8 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z"></path></svg>"#
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(html: &str) -> Cow<'_, str> {
        transform_gfm_alerts(html, usize::MAX).unwrap()
    }

    /// The pre-review scan: retries every `<blockquote>` (quadratic).
    fn exhaustive(html: &str) -> String {
        let mut out = String::new();
        let (mut copied, mut search) = (0, 0);
        while let Some(start) = find_ci(html, search, "<blockquote>") {
            if let Outcome::Match(end, replacement) = match_alert(html, start) {
                out.push_str(&html[copied..start]);
                out.push_str(&replacement);
                (copied, search) = (end, end);
            } else {
                search = start + 1;
            }
        }
        out.push_str(&html[copied..]);
        out
    }

    #[test]
    fn lowercase_marker_keeps_written_title() {
        let html = run("<blockquote>\n<p>[!note]<br />\nText</p>\n<p>More</p>\n</blockquote>\n");
        assert!(html.starts_with("<div class=\"markdown-alert markdown-alert-note\">"));
        assert!(html.ends_with("<span>note</span></div><p>Text</p><p>More</p></div>\n"));
    }

    #[test]
    fn non_alerts_and_unknown_kinds_are_untouched() {
        for html in [
            "<blockquote>\n<p>plain</p>\n</blockquote>\n",
            "<blockquote>\n<p>[!DANGER]\nx</p>\n</blockquote>\n",
            "<blockquote>\n<p>[!NOTE]\nunterminated</p>\n",
        ] {
            assert!(matches!(run(html), Cow::Borrowed(text) if text == html));
        }
    }

    #[test]
    fn empty_alert_body_omits_paragraph() {
        let html = run("<blockquote>\n<p>[!TIP]</p>\n</blockquote>\n");
        assert!(html.ends_with("<span>TIP</span></div></div>\n"), "{html}");
    }

    #[test]
    fn early_stop_matches_exhaustive_scan() {
        for html in [
            "<blockquote><p>[!NOTE]</p>".repeat(20),
            "<blockquote><p>[!NOTE]<blockquote><p>[!TIP] x</p>".to_owned(),
            "<blockquote><p>[!NOTE] a</p></blockquote><blockquote><p>[!TIP] b".to_owned(),
            "<blockquote>\n<p>[!NOTE]\n<blockquote>\n<p>[!TIP] inner</p>\n</blockquote>\n</blockquote>\n"
                .to_owned(),
            "<blockquote><p>x</p><blockquote><p>[!WARNING] y</p></blockquote></blockquote>".to_owned(),
        ] {
            assert_eq!(run(&html), exhaustive(&html), "{html}");
        }
    }

    #[test]
    fn nested_quote_splice_is_parity_required_broken_nesting() {
        // .NET's lazy regex closes the alert at the inner `</blockquote>`,
        // leaving a stray outer close tag. The sanitizer's HTML5 parser must
        // repair it; a regex sanitizer must never be used downstream.
        let html = run(
            "<blockquote>\n<p>[!NOTE] a</p>\n<blockquote>\n<p>b</p>\n</blockquote>\n</blockquote>\n",
        );
        assert!(
            html.ends_with("<p>a</p><blockquote>\n<p>b</p></div>\n</blockquote>\n"),
            "{html}"
        );
    }

    #[test]
    fn output_limit_is_enforced() {
        let html = "<blockquote><p>[!NOTE] x</p></blockquote>";
        assert!(transform_gfm_alerts(html, 100).is_none());
        assert!(transform_gfm_alerts(html, 10_000).is_some());
    }
}
