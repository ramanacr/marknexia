//! The .NET `MarkdownRenderer` regex passes, as direct scanners with the
//! same leftmost-match and backtracking outcome. Every pass runs on markup
//! that has **not** been sanitized yet; the results only ever leave the
//! crate through `HtmlPolicy::sanitize_fragment`. (.NET's remote-image
//! blanking, which runs on sanitized markup, is the sanitizer's
//! `RemoteImagePolicy::Blank`.)
//!
//! Each pass takes the markup by value and hands it back unchanged, without a
//! copy, when nothing matched. Before building a replacement, a pass checks
//! its exact size (or, for highlighted code, an exact lower bound), derived
//! from the replacement's fixed template text, against the budget left.

use crate::{
    highlight, math,
    text::{
        escape_data_string, escaped_data_string_len, find_ci, html_decode, html_encode,
        html_encoded_len, skip_space, starts_with_ci,
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
            out: String::new(),
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
        if self.out.capacity() == 0 {
            self.out
                .reserve((self.html.len() + self.html.len() / 4).min(self.limit));
        }
        self.out.push_str(&self.html[self.copied_to..start]);
        self.out.push_str(replacement);
        self.copied_to = end;
        if self.out.len() > self.limit {
            return Err(Overflow);
        }
        Ok(())
    }

    /// The spliced markup, or `None` when nothing was replaced.
    fn finish(mut self) -> Result<Option<String>, Overflow> {
        if self.copied_to == 0 && self.out.is_empty() {
            return Ok(None);
        }
        self.out.push_str(&self.html[self.copied_to..]);
        if self.out.len() > self.limit {
            return Err(Overflow);
        }
        Ok(Some(self.out))
    }
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

/// The fixed text of a code-block replacement, around the copy metadata and
/// the highlighted block.
const COPY_OPEN: &str = "\n<div class=\"code-container\">\n  <button type=\"button\" class=\"copy-btn\" data-marknexia-action=\"copy\" data-copy-text=\"";
const COPY_MIDDLE: &str = "\" title=\"Copy code\">Copy</button>\n  ";
const COPY_CLOSE: &str = "\n</div>";

/// Exact lower bound of a code-block replacement's size (exact unless the
/// block is highlighted C#).
fn code_block_minimum(code: &str, language: &str) -> usize {
    COPY_OPEN.len()
        + escaped_data_string_len(code)
        + COPY_MIDDLE.len()
        + highlight::minimum_len(code, language)
        + COPY_CLOSE.len()
}

/// Step 2, `HighlightCodeBlocks`:
/// `<pre><code(?:\s+class="language-([a-zA-Z0-9_\+#\-]+)")?>([\s\S]*?)</code></pre>`.
pub(crate) fn highlight_code_blocks(html: String, limit: usize) -> Result<String, Overflow> {
    let spliced = highlight_code_blocks_in(&html, limit)?;
    Ok(spliced.unwrap_or(html))
}

fn highlight_code_blocks_in(html: &str, limit: usize) -> Result<Option<String>, Overflow> {
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
            let remaining = splicer.remaining(start)?;
            if code_block_minimum(&code, language) > remaining {
                return Err(Overflow);
            }
            let highlighted = highlight::highlight(&code, language, remaining).ok_or(Overflow)?;
            let mut replacement = String::with_capacity(
                COPY_OPEN.len()
                    + escaped_data_string_len(&code)
                    + COPY_MIDDLE.len()
                    + highlighted.len()
                    + COPY_CLOSE.len(),
            );
            replacement.push_str(COPY_OPEN);
            escape_data_string(&code, &mut replacement);
            replacement.push_str(COPY_MIDDLE);
            replacement.push_str(&highlighted);
            replacement.push_str(COPY_CLOSE);
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
    html: String,
    settings: DiagramSettings,
    limit: usize,
) -> Result<(String, bool), Overflow> {
    let (spliced, has_mermaid) = transform_diagrams_in(&html, settings, limit)?;
    Ok((spliced.unwrap_or(html), has_mermaid))
}

fn transform_diagrams_in(
    html: &str,
    settings: DiagramSettings,
    limit: usize,
) -> Result<(Option<String>, bool), Overflow> {
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
        let reason = if !settings.enabled {
            Some("diagram rendering is disabled for this document")
        } else if counter > settings.max_count {
            Some("diagram limit exceeded")
        } else if source.len() > settings.max_source_bytes {
            Some("diagram source limit exceeded")
        } else {
            None
        };
        let id = format!("mermaid-{counter}");
        // Exact sizes, from the fixed template text.
        let size = match reason {
            Some(reason) => diagram_fallback_len(&source, reason),
            None => mermaid_shell_len(&source, &id),
        };
        if size > splicer.remaining(start)? {
            return Err(Overflow);
        }
        let replacement = match reason {
            Some(reason) => diagram_fallback(&source, reason),
            None => {
                has_mermaid = true;
                mermaid_shell(&source, &id)
            }
        };
        splicer.replace(start, end, &replacement)?;
        search = end;
    }
    Ok((splicer.finish()?, has_mermaid))
}

/// `MarkdownRenderer.RenderDiagramFallback`, around the reason and the
/// source.
const FALLBACK_OPEN: &str = "\n<div class=\"marknexia-diagram-fallback\" role=\"note\"><strong>Mermaid diagram was not rendered.</strong>\n  <p>";
const FALLBACK_MIDDLE: &str = ".</p>\n  <details><summary>Show diagram source</summary><pre><code>";
const FALLBACK_CLOSE: &str = "</code></pre></details>\n</div>";

fn diagram_fallback_len(source: &str, reason: &str) -> usize {
    FALLBACK_OPEN.len()
        + html_encoded_len(reason)
        + FALLBACK_MIDDLE.len()
        + html_encoded_len(source)
        + FALLBACK_CLOSE.len()
}

fn diagram_fallback(source: &str, reason: &str) -> String {
    let mut out = String::with_capacity(diagram_fallback_len(source, reason));
    out.push_str(FALLBACK_OPEN);
    html_encode(reason, &mut out);
    out.push_str(FALLBACK_MIDDLE);
    html_encode(source, &mut out);
    out.push_str(FALLBACK_CLOSE);
    out
}

/// Placeholders of [`SHELL_TEMPLATE`] and how often each occurs.
const ID: (&str, usize) = ("{id}", 7);
const ESCAPED: (&str, usize) = ("{escaped}", 1);
const ENCODED: (&str, usize) = ("{encoded}", 3);

/// Bytes of [`SHELL_TEMPLATE`] outside its placeholders.
const SHELL_FIXED: usize = SHELL_TEMPLATE.len()
    - ID.0.len() * ID.1
    - ESCAPED.0.len() * ESCAPED.1
    - ENCODED.0.len() * ENCODED.1;

fn mermaid_shell_len(source: &str, id: &str) -> usize {
    SHELL_FIXED
        + id.len() * ID.1
        + escaped_data_string_len(source) * ESCAPED.1
        + html_encoded_len(source) * ENCODED.1
}

/// `MermaidDiagramRenderer.RenderDiagramToHtml`. `id` is generated
/// (`mermaid-N`) and needs no encoding.
fn mermaid_shell(source: &str, id: &str) -> String {
    let mut encoded = String::with_capacity(html_encoded_len(source));
    html_encode(source, &mut encoded);
    let mut escaped = String::with_capacity(escaped_data_string_len(source));
    escape_data_string(source, &mut escaped);
    let values = [
        (ID.0, id),
        (ESCAPED.0, escaped.as_str()),
        (ENCODED.0, encoded.as_str()),
    ];
    let mut out = String::with_capacity(mermaid_shell_len(source, id));
    let mut rest = SHELL_TEMPLATE;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let (placeholder, value) = values
            .iter()
            .copied()
            .find(|(placeholder, _)| rest.starts_with(placeholder))
            .unwrap_or(("{", "{"));
        out.push_str(value);
        rest = &rest[placeholder.len()..];
    }
    out.push_str(rest);
    out
}

/// The shell's markup; `{id}`, `{escaped}` (the percent-encoded source) and
/// `{encoded}` (the entity-encoded source) are filled in.
const SHELL_TEMPLATE: &str = "\n<div class=\"marknexia-diagram marknexia-mermaid\" id=\"{id}\" data-diagram-type=\"mermaid\">
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
</div>";

/// Step 3b, `RenderMath`, case-insensitive:
/// `<(span|div)\s+class="math">(\s*(?:\\\(|\\\[|\$\$)[\s\S]*?(?:\\\)|\\\]|\$\$)\s*)</\k<tag>>`.
pub(crate) fn render_math(html: String, enabled: bool, limit: usize) -> Result<String, Overflow> {
    let spliced = render_math_in(&html, enabled, limit)?;
    Ok(spliced.unwrap_or(html))
}

fn render_math_in(html: &str, enabled: bool, limit: usize) -> Result<Option<String>, Overflow> {
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
            if math::fallback_len(expression, display) > remaining {
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
        let out = highlight_code_blocks(html.to_owned(), usize::MAX).unwrap();
        assert!(out.starts_with("\n<div class=\"code-container\">\n  <button type=\"button\" class=\"copy-btn\" data-marknexia-action=\"copy\" data-copy-text=\"var%20value%20%3D%201%3B%0A\" title=\"Copy code\">Copy</button>\n  <div class=\"csharp\"><pre>\n<span class=\"keyword\">var</span>"));
        assert!(out.ends_with("\n<pre><code class=\"language-Mermaid\">a</code></pre>"));
        // Case-sensitive, and an unsupported class character leaves the block.
        let raw = "<PRE><CODE>x</CODE></PRE><pre><code class=\"language-a.b\">y</code></pre>";
        assert_eq!(
            highlight_code_blocks(raw.to_owned(), usize::MAX).unwrap(),
            raw
        );
        // Unterminated blocks are left alone.
        assert_eq!(
            highlight_code_blocks("<pre><code>x".to_owned(), usize::MAX).unwrap(),
            "<pre><code>x"
        );
    }

    #[test]
    fn diagram_pass_counts_and_limits() {
        let html = "<pre><code class=\"language-mermaid\">a--&gt;b\n</code></pre><PRE><CODE  CLASS=\"LANGUAGE-MERMAID\">c</CODE></PRE>";
        let (out, has_mermaid) = transform_diagrams(html.to_owned(), DIAGRAMS, usize::MAX).unwrap();
        assert!(has_mermaid);
        assert!(out.contains("id=\"mermaid-1\""));
        assert!(out.contains("data-copy-text=\"a--%3Eb%0A\""));
        assert!(out.contains("<p>diagram limit exceeded.</p>"));
        let disabled = DiagramSettings {
            enabled: false,
            ..DIAGRAMS
        };
        let (out, has_mermaid) = transform_diagrams(html.to_owned(), disabled, usize::MAX).unwrap();
        assert!(!has_mermaid);
        assert_eq!(out.matches("diagram rendering is disabled").count(), 2);
    }

    #[test]
    fn math_pass() {
        let html =
            "<p><span class=\"math\">\\(x^2\\)</span> <DIV class=\"math\"> $$a\\)b$$ </div></p>";
        let out = render_math(html.to_owned(), true, usize::MAX).unwrap();
        assert_eq!(
            out,
            "<p><span class=\"marknexia-math\" role=\"math\" aria-label=\"x^2\">x<sup>2</sup></span> <div class=\"marknexia-math-display\" role=\"math\" aria-label=\"$$a\\)b$$\">$$a)b$$</div></p>"
        );
        assert_eq!(
            render_math(
                "<span class=\"math\">\\(a\\)</span>".to_owned(),
                false,
                usize::MAX
            )
            .unwrap(),
            "<span class=\"marknexia-math-fallback\" role=\"math\">\\(a\\)</span>"
        );
        let unterminated = "<span class=\"math\">\\(a".repeat(10_000);
        assert_eq!(
            render_math(unterminated.clone(), true, usize::MAX).unwrap(),
            unterminated
        );
    }

    /// The pass accepts exactly the budget its output needs, and rejects one
    /// byte less (review M5: the estimates used to be 7 to 54 bytes too big).
    fn assert_exact_edge(pass: impl Fn(usize) -> Result<String, Overflow>) {
        let needed = pass(usize::MAX).unwrap().len();
        assert_eq!(pass(needed).map(|out| out.len()), Ok(needed));
        assert_eq!(pass(needed - 1), Err(Overflow));
    }

    #[test]
    fn code_block_budget_edges_are_exact() {
        for block in [
            "<pre><code>plain &amp; &lt;text&gt;\n</code></pre>",
            "<pre><code class=\"language-c#\">var x = 1; // c\n</code></pre>",
            "<pre><code class=\"language-sh\"></code></pre>",
            "<pre><code></code></pre>",
            "<pre><code>é😀 %20\n</code></pre>",
        ] {
            assert_exact_edge(|limit| highlight_code_blocks(block.to_owned(), limit));
            // The lower bound is exact for plain blocks and never above the
            // real size.
            let open = block.find("<code").unwrap();
            let start = open + block[open..].find('>').unwrap() + 1;
            let code = html_decode(&block[start..block.find("</code>").unwrap()]);
            let language = if block.contains("c#") { "c#" } else { "" };
            let real = highlight_code_blocks(block.to_owned(), usize::MAX)
                .unwrap()
                .len();
            let minimum = code_block_minimum(&code, language);
            assert!(minimum <= real, "{block}");
            if language.is_empty() {
                assert_eq!(minimum, real, "{block}");
            }
        }
    }

    #[test]
    fn diagram_budget_edges_are_exact() {
        let block =
            "<pre><code class=\"language-mermaid\">graph TD\nA--&gt;B &amp; \"q\" é\n</code></pre>";
        for settings in [
            DIAGRAMS,
            DiagramSettings {
                enabled: false,
                ..DIAGRAMS
            },
            DiagramSettings {
                max_source_bytes: 4,
                ..DIAGRAMS
            },
        ] {
            assert_exact_edge(|limit| {
                transform_diagrams(block.to_owned(), settings, limit).map(|(out, _)| out)
            });
        }
        // Two diagrams: the second one exceeds the count.
        let two = format!("{block}{block}");
        assert_exact_edge(|limit| {
            transform_diagrams(two.clone(), DIAGRAMS, limit).map(|(out, _)| out)
        });
        // The size functions are the template's exact lengths.
        let source = "a<b>&\"'c é";
        assert_eq!(
            mermaid_shell(source, "mermaid-12").len(),
            mermaid_shell_len(source, "mermaid-12")
        );
        assert_eq!(
            SHELL_TEMPLATE.matches(ID.0).count() * 1_000
                + SHELL_TEMPLATE.matches(ESCAPED.0).count() * 100
                + SHELL_TEMPLATE.matches(ENCODED.0).count(),
            ID.1 * 1_000 + ESCAPED.1 * 100 + ENCODED.1
        );
        assert_eq!(
            diagram_fallback(source, "why").len(),
            diagram_fallback_len(source, "why")
        );
    }

    #[test]
    fn math_budget_edges_are_exact() {
        let html =
            "<p><span class=\"math\">\\(x^2 &lt; y\\)</span><div class=\"math\">$$a$$</div></p>";
        assert_exact_edge(|limit| render_math(html.to_owned(), true, limit));
        assert_exact_edge(|limit| render_math(html.to_owned(), false, limit));
    }

    #[test]
    fn unchanged_markup_is_returned_without_a_copy() {
        let html = String::from("<p>no code, diagrams or math</p>");
        let pointer = html.as_ptr();
        let html = highlight_code_blocks(html, usize::MAX).unwrap();
        let (html, _) = transform_diagrams(html, DIAGRAMS, usize::MAX).unwrap();
        let html = render_math(html, true, usize::MAX).unwrap();
        assert_eq!(html.as_ptr(), pointer);
    }
}
