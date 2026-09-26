//! The .NET `MarkdownRenderer` regex passes, as direct scanners with the
//! same leftmost-match and backtracking outcome. Every pass runs on markup
//! that has **not** been sanitized yet (except [`block_remote_images`]); the
//! results only ever leave the crate through `HtmlPolicy::sanitize_fragment`.

use crate::{
    highlight, math,
    text::{
        escape_data_string, find_ci, html_decode, html_encode, is_space, is_word, skip_space,
        starts_with_ci,
    },
};

/// A pass's output would exceed its byte budget. Passes check an estimate
/// before building each replacement, so no intermediate string grows far
/// past the budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Overflow;

/// Splices replacements into `html` as a `Regex.Replace` would, within a
/// byte budget for the whole output.
struct Splicer<'a> {
    html: &'a str,
    out: String,
    copied_to: usize,
    limit: usize,
}

impl<'a> Splicer<'a> {
    fn new(html: &'a str, limit: usize) -> Self {
        Self {
            html,
            out: String::with_capacity((html.len() + html.len() / 4).min(limit)),
            copied_to: 0,
            limit,
        }
    }

    /// Bytes a replacement starting at `start` may take.
    fn remaining(&self, start: usize) -> Result<usize, Overflow> {
        self.limit
            .checked_sub(self.out.len() + (start - self.copied_to))
            .ok_or(Overflow)
    }

    fn replace(&mut self, start: usize, end: usize, replacement: &str) -> Result<(), Overflow> {
        self.out.push_str(&self.html[self.copied_to..start]);
        self.out.push_str(replacement);
        self.copied_to = end;
        if self.out.len() > self.limit {
            return Err(Overflow);
        }
        Ok(())
    }

    fn finish(mut self) -> Result<String, Overflow> {
        self.out.push_str(&self.html[self.copied_to..]);
        if self.out.len() > self.limit {
            return Err(Overflow);
        }
        Ok(self.out)
    }
}

/// Blanking passes only shrink markup, so they cannot overflow.
fn finish_unbounded(splicer: Splicer<'_>) -> String {
    splicer.finish().unwrap_or_default()
}

/// `\s+class="language-([a-zA-Z0-9_\+#\-]+)"` at `from`: the language and the
/// offset after the closing quote.
fn language_class(html: &str, from: usize, ignore_case: bool) -> Option<(&str, usize)> {
    let class = skip_space(html, from);
    if class == from {
        return None;
    }
    const PREFIX: &str = "class=\"language-";
    let matches = if ignore_case {
        starts_with_ci(html, class, PREFIX)
    } else {
        html[class..].starts_with(PREFIX)
    };
    if !matches {
        return None;
    }
    let start = class + PREFIX.len();
    let end = html[start..]
        .bytes()
        .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'#' | b'-')))
        .map_or(html.len(), |offset| start + offset);
    (end > start && html.as_bytes().get(end) == Some(&b'"')).then(|| (&html[start..end], end + 1))
}

/// Step 2, `HighlightCodeBlocks`:
/// `<pre><code(?:\s+class="language-([a-zA-Z0-9_\+#\-]+)")?>([\s\S]*?)</code></pre>`.
pub(crate) fn highlight_code_blocks(html: &str, limit: usize) -> Result<String, Overflow> {
    const OPEN: &str = "<pre><code";
    const CLOSE: &str = "</code></pre>";
    let mut splicer = Splicer::new(html, limit);
    let mut search = 0;
    while let Some(offset) = html[search..].find(OPEN) {
        let start = search + offset;
        let after = start + OPEN.len();
        let head = language_class(html, after, false)
            .filter(|&(_, end)| html.as_bytes().get(end) == Some(&b'>'))
            .map(|(language, end)| (language, end + 1))
            .or_else(|| (html.as_bytes().get(after) == Some(&b'>')).then_some(("", after + 1)));
        let Some((language, content_start)) = head else {
            search = start + 1;
            continue;
        };
        let Some(close) = html[content_start..].find(CLOSE) else {
            break;
        };
        let content_end = content_start + close;
        let end = content_end + CLOSE.len();
        if !language.eq_ignore_ascii_case("mermaid") {
            let code = html_decode(&html[content_start..content_end]);
            // The copy metadata and the highlighted code each take at least
            // `code.len()` bytes.
            let remaining = splicer.remaining(start)?;
            if code.len().saturating_mul(2).saturating_add(192) > remaining {
                return Err(Overflow);
            }
            let highlighted = highlight::highlight(&code, language, remaining).ok_or(Overflow)?;
            let mut replacement = String::with_capacity(code.len() * 3 + highlighted.len() + 192);
            replacement.push_str("\n<div class=\"code-container\">\n  <button type=\"button\" class=\"copy-btn\" data-marknexia-action=\"copy\" data-copy-text=\"");
            escape_data_string(&code, &mut replacement);
            replacement.push_str("\" title=\"Copy code\">Copy</button>\n  ");
            replacement.push_str(&highlighted);
            replacement.push_str("\n</div>");
            splicer.replace(start, end, &replacement)?;
        }
        search = end;
    }
    splicer.finish()
}

/// Diagram limits and switches for step 3.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DiagramSettings {
    pub enabled: bool,
    pub max_count: usize,
    pub max_source_bytes: usize,
}

/// Step 3: every `<pre><code\s+class="language-mermaid">…</code></pre>`
/// (case-insensitive) becomes a diagram shell or a fallback note. Returns the
/// new markup and whether any diagram shell was emitted (`hasMermaid`).
pub(crate) fn transform_diagrams(
    html: &str,
    settings: DiagramSettings,
    limit: usize,
) -> Result<(String, bool), Overflow> {
    const OPEN: &str = "<pre><code";
    const CLOSE: &str = "</code></pre>";
    let mut splicer = Splicer::new(html, limit);
    let mut search = 0;
    let mut counter = 0_usize;
    let mut has_mermaid = false;
    while let Some(start) = find_ci(html, search, OPEN) {
        let after = start + OPEN.len();
        let content_start = language_class(html, after, true)
            .filter(|&(language, end)| {
                language.eq_ignore_ascii_case("mermaid") && html.as_bytes().get(end) == Some(&b'>')
            })
            .map(|(_, end)| end + 1);
        let Some(content_start) = content_start else {
            search = start + 1;
            continue;
        };
        let Some(content_end) = find_ci(html, content_start, CLOSE) else {
            break;
        };
        let end = content_end + CLOSE.len();
        let source = html_decode(&html[content_start..content_end]);
        counter += 1;
        let shell = settings.enabled
            && counter <= settings.max_count
            && source.len() <= settings.max_source_bytes;
        // Lower bounds: a shell holds the source three times plus its
        // percent-encoded copy; a fallback holds it once.
        let estimate = if shell {
            source.len().saturating_mul(4).saturating_add(2_048)
        } else {
            source.len().saturating_add(256)
        };
        if estimate > splicer.remaining(start)? {
            return Err(Overflow);
        }
        let replacement = if !settings.enabled {
            diagram_fallback(&source, "diagram rendering is disabled for this document")
        } else if counter > settings.max_count {
            diagram_fallback(&source, "diagram limit exceeded")
        } else if source.len() > settings.max_source_bytes {
            diagram_fallback(&source, "diagram source limit exceeded")
        } else {
            has_mermaid = true;
            mermaid_shell(&source, &format!("mermaid-{counter}"))
        };
        splicer.replace(start, end, &replacement)?;
        search = end;
    }
    Ok((splicer.finish()?, has_mermaid))
}

/// `MarkdownRenderer.RenderDiagramFallback`.
fn diagram_fallback(source: &str, reason: &str) -> String {
    let mut out = String::with_capacity(source.len() + 256);
    out.push_str("\n<div class=\"marknexia-diagram-fallback\" role=\"note\"><strong>Mermaid diagram was not rendered.</strong>\n  <p>");
    html_encode(reason, &mut out);
    out.push_str(".</p>\n  <details><summary>Show diagram source</summary><pre><code>");
    html_encode(source, &mut out);
    out.push_str("</code></pre></details>\n</div>");
    out
}

/// `MermaidDiagramRenderer.RenderDiagramToHtml`. `id` is generated
/// (`mermaid-N`) and needs no encoding.
fn mermaid_shell(source: &str, id: &str) -> String {
    let encoded = crate::text::html_encoded(source);
    let mut escaped = String::with_capacity(source.len() * 3);
    escape_data_string(source, &mut escaped);
    format!(
        "\n<div class=\"marknexia-diagram marknexia-mermaid\" id=\"{id}\" data-diagram-type=\"mermaid\">
  <div class=\"marknexia-diagram-toolbar\">
    <span class=\"marknexia-diagram-label\">Mermaid Diagram</span>
    <div class=\"marknexia-diagram-actions\">
      <button type=\"button\" class=\"marknexia-btn marknexia-btn-copy\" data-marknexia-action=\"copy\" data-copy-text=\"{escaped}\" title=\"Copy Diagram Source\">
        Copy
      </button>
      <button type=\"button\" class=\"marknexia-btn marknexia-btn-toggle\" data-marknexia-action=\"toggle-source\" title=\"Toggle Diagram Source\">
        Source
      </button>
      <button type=\"button\" class=\"marknexia-btn\" data-marknexia-action=\"zoom-out\" title=\"Zoom out\" aria-label=\"Zoom out\">\u{2212}</button>
      <button type=\"button\" class=\"marknexia-btn marknexia-diagram-zoom-reset\" data-marknexia-action=\"zoom-reset\" title=\"Reset diagram zoom\" aria-label=\"Reset diagram zoom\">100%</button>
      <button type=\"button\" class=\"marknexia-btn\" data-marknexia-action=\"zoom-in\" title=\"Zoom in\" aria-label=\"Zoom in\">+</button>
      <button type=\"button\" class=\"marknexia-btn\" data-marknexia-action=\"expand\" title=\"Open full-window diagram\" aria-label=\"Open diagram full window\">\u{26f6}</button>
    </div>
  </div>
  <div class=\"marknexia-diagram-viewport\" id=\"{id}-viewport\">
    <div class=\"marknexia-diagram-canvas\" id=\"{id}-canvas\"><pre class=\"mermaid\" id=\"{id}-render\">{encoded}</pre></div>
  </div>
  <div class=\"marknexia-diagram-zoom-status\" data-marknexia-zoom-status aria-live=\"polite\">100%</div>
  <div class=\"marknexia-diagram-source\" id=\"{id}-source\" style=\"display: none;\">
    <pre><code>{encoded}</code></pre>
  </div>
  <div class=\"marknexia-diagram-error\" id=\"{id}-error\" style=\"display: none;\">
    <div class=\"marknexia-diagram-error-title\">Diagram failed to render</div>
    <div class=\"marknexia-diagram-error-msg\" id=\"{id}-error-msg\"></div>
    <div class=\"marknexia-diagram-error-source\">
      <pre><code>{encoded}</code></pre>
    </div>
  </div>
</div>"
    )
}

/// Step 3b, `RenderMath`, case-insensitive:
/// `<(span|div)\s+class="math">(\s*(?:\\\(|\\\[|\$\$)[\s\S]*?(?:\\\)|\\\]|\$\$)\s*)</\k<tag>>`.
pub(crate) fn render_math(html: &str, enabled: bool, limit: usize) -> Result<String, Overflow> {
    let mut splicer = Splicer::new(html, limit);
    // Lazy-body failures are monotone per tag: no match from offset `n`
    // means none from any later offset either.
    let mut dead_from = [usize::MAX; 2];
    let mut search = 0;
    while let Some(offset) = html[search..].find('<') {
        let start = search + offset;
        search = start + 1;
        let (tag_index, tag) = if starts_with_ci(html, start + 1, "span") {
            (0, "span")
        } else if starts_with_ci(html, start + 1, "div") {
            (1, "div")
        } else {
            continue;
        };
        let after_tag = start + 1 + tag.len();
        let class = skip_space(html, after_tag);
        if class == after_tag || !starts_with_ci(html, class, "class=\"math\">") {
            continue;
        }
        let expression_start = class + "class=\"math\">".len();
        let opener = skip_space(html, expression_start);
        let opens = ["\\(", "\\[", "$$"]
            .iter()
            .any(|open| html[opener..].starts_with(open));
        if !opens {
            continue;
        }
        let body = opener + 2;
        if body >= dead_from[tag_index] {
            continue;
        }
        let Some((expression_end, end)) = math_close(html, body, tag) else {
            dead_from[tag_index] = body;
            continue;
        };
        let expression = &html[expression_start..expression_end];
        let display = {
            let trimmed = expression.trim();
            trimmed.starts_with("\\[") || trimmed.starts_with("$$")
        };
        let remaining = splicer.remaining(start)?;
        let replacement = if enabled {
            math::render(expression, display, remaining).ok_or(Overflow)?
        } else {
            if expression.len().saturating_add(64) > remaining {
                return Err(Overflow);
            }
            math::fallback(expression, display)
        };
        splicer.replace(start, end, &replacement)?;
        search = end;
    }
    splicer.finish()
}

/// The first closer at or after `body` followed by `\s*</tag>`: the end of
/// the expression group and of the whole match.
fn math_close(html: &str, body: usize, tag: &str) -> Option<(usize, usize)> {
    let bytes = html.as_bytes();
    let mut index = body;
    while index + 1 < bytes.len() {
        let pair = &bytes[index..index + 2];
        if pair == b"\\)" || pair == b"\\]" || pair == b"$$" {
            let expression_end = skip_space(html, index + 2);
            let close = expression_end;
            if html[close..].starts_with("</")
                && starts_with_ci(html, close + 2, tag)
                && bytes.get(close + 2 + tag.len()) == Some(&b'>')
            {
                return Some((expression_end, close + 3 + tag.len()));
            }
        }
        index += 1;
    }
    None
}

/// .NET `BlockRemoteImages`, applied to **sanitized** markup (as in .NET):
/// a quoted `src` starting with `http://`, `https://` or `//` becomes
/// `src=""`, and a quoted `srcset` naming such a URL is removed.
pub(crate) fn block_remote_images(html: &str) -> String {
    blank_remote_srcset(&blank_remote_src(html))
}

/// `<img\b(?<attributes>[^>]*?)\bsrc\s*=\s*(?<quote>["'])(?:https?://|//)[^"']*\k<quote>(?<tail>[^>]*)>`
/// → `<img{attributes}src=""{tail}>` (case-insensitive).
fn blank_remote_src(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut splicer = Splicer::new(html, usize::MAX);
    let mut search = 0;
    // Every `src` candidate before this offset already failed: candidate
    // checks depend only on their own position.
    let mut failed_until = 0;
    while let Some(start) = find_ci(html, search, "<img") {
        search = start + 1;
        let attributes = start + 4;
        if html[attributes..].chars().next().is_none_or(is_word) {
            continue;
        }
        let mut candidate = attributes.max(failed_until);
        let found = loop {
            match bytes.get(candidate) {
                None => return finish_unbounded(splicer),
                Some(b'>') => break None,
                _ => {}
            }
            if let Some(tail) = remote_src_at(html, candidate) {
                // `(?<tail>[^>]*)>`: without a later `>` no candidate here or
                // in any later `<img` can complete.
                let Some(close) = html[tail..].find('>') else {
                    return finish_unbounded(splicer);
                };
                break Some((candidate, tail, tail + close));
            }
            candidate += html[candidate..].chars().next().map_or(1, char::len_utf8);
        };
        let Some((src, tail, close)) = found else {
            failed_until = candidate;
            continue;
        };
        let mut replacement = String::with_capacity(close + 1 - start);
        replacement.push_str("<img");
        replacement.push_str(&html[attributes..src]);
        replacement.push_str("src=\"\"");
        replacement.push_str(&html[tail..close]);
        replacement.push('>');
        let _ = splicer.replace(start, close + 1, &replacement);
        search = close + 1;
    }
    finish_unbounded(splicer)
}

/// `\bsrc\s*=\s*(["'])(?:https?://|//)[^"']*\1` at `at`: offset after it.
fn remote_src_at(html: &str, at: usize) -> Option<usize> {
    if !starts_with_ci(html, at, "src") || html[..at].chars().next_back().is_some_and(is_word) {
        return None;
    }
    let equals = skip_space(html, at + 3);
    if html.as_bytes().get(equals) != Some(&b'=') {
        return None;
    }
    let quote_at = skip_space(html, equals + 1);
    let quote = *html
        .as_bytes()
        .get(quote_at)
        .filter(|q| matches!(q, b'"' | b'\''))?;
    let url = quote_at + 1;
    let scheme_end = ["https://", "http://", "//"]
        .iter()
        .find(|scheme| starts_with_ci(html, url, scheme))
        .map(|scheme| url + scheme.len())?;
    let value_end = html[scheme_end..]
        .find(['"', '\''])
        .map(|offset| scheme_end + offset)?;
    (html.as_bytes()[value_end] == quote).then_some(value_end + 1)
}

/// `\s+srcset\s*=\s*(?<quote>["'])(?=[^"']*(?:https?://|//))[^"']*\k<quote>`
/// → removed (case-insensitive).
fn blank_remote_srcset(html: &str) -> String {
    let mut splicer = Splicer::new(html, usize::MAX);
    let mut search = 0;
    while let Some(found) = find_ci(html, search, "srcset") {
        search = found + 1;
        // Leftmost match: the start of the white-space run before `srcset`,
        // but not inside the previous replacement.
        let floor = splicer.copied_to;
        if found <= floor {
            continue;
        }
        let run_start = html[floor..found]
            .char_indices()
            .rev()
            .take_while(|&(_, c)| is_space(c))
            .last()
            .map(|(index, _)| floor + index);
        let Some(start) = run_start else {
            continue;
        };
        let equals = skip_space(html, found + 6);
        if html.as_bytes().get(equals) != Some(&b'=') {
            continue;
        }
        let quote_at = skip_space(html, equals + 1);
        let Some(&quote) = html
            .as_bytes()
            .get(quote_at)
            .filter(|q| matches!(q, b'"' | b'\''))
        else {
            continue;
        };
        let value = quote_at + 1;
        let Some(value_end) = html[value..].find(['"', '\'']).map(|offset| value + offset) else {
            continue;
        };
        if html.as_bytes()[value_end] != quote || !html[value..value_end].contains("//") {
            continue;
        }
        let _ = splicer.replace(start, value_end + 1, "");
        search = value_end + 1;
    }
    finish_unbounded(splicer)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIAGRAMS: DiagramSettings = DiagramSettings {
        enabled: true,
        max_count: 1,
        max_source_bytes: 1000,
    };

    #[test]
    fn highlight_pass_wraps_code_and_skips_mermaid() {
        let html = "<pre><code class=\"language-c#\">var value = 1;\n</code></pre>\n<pre><code class=\"language-Mermaid\">a</code></pre>";
        let out = highlight_code_blocks(html, usize::MAX).unwrap();
        assert!(out.starts_with("\n<div class=\"code-container\">\n  <button type=\"button\" class=\"copy-btn\" data-marknexia-action=\"copy\" data-copy-text=\"var%20value%20%3D%201%3B%0A\" title=\"Copy code\">Copy</button>\n  <div class=\"csharp\"><pre>\n<span class=\"keyword\">var</span>"));
        assert!(out.ends_with("\n<pre><code class=\"language-Mermaid\">a</code></pre>"));
        // Case-sensitive, and an unsupported class character leaves the block.
        let raw = "<PRE><CODE>x</CODE></PRE><pre><code class=\"language-a.b\">y</code></pre>";
        assert_eq!(highlight_code_blocks(raw, usize::MAX).unwrap(), raw);
        // Unterminated blocks are left alone.
        assert_eq!(
            highlight_code_blocks("<pre><code>x", usize::MAX).unwrap(),
            "<pre><code>x"
        );
    }

    #[test]
    fn diagram_pass_counts_and_limits() {
        let html = "<pre><code class=\"language-mermaid\">a--&gt;b\n</code></pre><PRE><CODE  CLASS=\"LANGUAGE-MERMAID\">c</CODE></PRE>";
        let (out, has_mermaid) = transform_diagrams(html, DIAGRAMS, usize::MAX).unwrap();
        assert!(has_mermaid);
        assert!(out.contains("id=\"mermaid-1\""));
        assert!(out.contains("data-copy-text=\"a--%3Eb%0A\""));
        assert!(out.contains("<p>diagram limit exceeded.</p>"));
        let disabled = DiagramSettings {
            enabled: false,
            ..DIAGRAMS
        };
        let (out, has_mermaid) = transform_diagrams(html, disabled, usize::MAX).unwrap();
        assert!(!has_mermaid);
        assert_eq!(out.matches("diagram rendering is disabled").count(), 2);
    }

    #[test]
    fn math_pass() {
        let html =
            "<p><span class=\"math\">\\(x^2\\)</span> <DIV class=\"math\"> $$a\\)b$$ </div></p>";
        let out = render_math(html, true, usize::MAX).unwrap();
        assert_eq!(
            out,
            "<p><span class=\"marknexia-math\" role=\"math\" aria-label=\"x^2\">x<sup>2</sup></span> <div class=\"marknexia-math-display\" role=\"math\" aria-label=\"$$a\\)b$$\">$$a)b$$</div></p>"
        );
        assert_eq!(
            render_math("<span class=\"math\">\\(a\\)</span>", false, usize::MAX).unwrap(),
            "<span class=\"marknexia-math-fallback\" role=\"math\">\\(a\\)</span>"
        );
        let unterminated = "<span class=\"math\">\\(a".repeat(10_000);
        assert_eq!(
            render_math(&unterminated, true, usize::MAX).unwrap(),
            unterminated
        );
    }

    #[test]
    fn remote_images_blank_like_dotnet() {
        assert_eq!(
            block_remote_images(
                "<p><img src=\"https://example.test/image.png\" alt=\"remote\"></p>"
            ),
            "<p><img src=\"\" alt=\"remote\"></p>"
        );
        assert_eq!(
            block_remote_images("<IMG alt=\"x\" SRC = '//h/a' title=\"t\"><img src=\"local.png\">"),
            "<img alt=\"x\" src=\"\" title=\"t\"><img src=\"local.png\">"
        );
        assert_eq!(
            block_remote_images(
                "<img src=\"a.png\" srcset=\"https://h/a 2x\"><img srcset=\"b 2x\">"
            ),
            "<img src=\"a.png\"><img srcset=\"b 2x\">"
        );
        assert_eq!(
            block_remote_images("<imgx src=\"https://h\">"),
            "<imgx src=\"https://h\">"
        );
    }
}
