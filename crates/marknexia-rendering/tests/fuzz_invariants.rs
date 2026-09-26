//! Deterministic stand-in for `fuzz/fuzz_targets/markdown_and_html.rs`: the
//! same invariants over pseudo-random token soups, so they are checked on
//! every `cargo test` (libFuzzer runs need the ASan runtime).

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, RenderLimits, Renderer};
use marknexia_security::{
    ContentPolicy, HtmlPolicy, MAX_HTML_OUTPUT_BYTES, PolicyLimits, RemoteImagePolicy,
};

const TOKENS: &[&str] = &[
    "```cs\n",
    "```mermaid\n",
    "```\n",
    "\n",
    "\n\n",
    " ",
    "# ",
    "> ",
    "- ",
    "*",
    "**",
    "`",
    "$",
    "$$",
    "\\(",
    "\\)",
    "\\frac",
    "\\sqrt",
    "^",
    "_",
    "{",
    "}",
    "[",
    "]",
    "(",
    ")",
    "<",
    ">",
    "&",
    "&amp;",
    "&#x41;",
    "&copy;",
    "\"",
    "'",
    "@\"",
    "/*",
    "*/",
    "//",
    "///",
    "#if",
    "var",
    "1",
    "x",
    "é",
    "😀",
    "<pre><code class=\"language-mermaid\">",
    "</code></pre>",
    "<pre><code class=\"language-c#\">",
    "<span class=\"math\">",
    "</span>",
    "<div>",
    "</div>",
    "<img src=\"https://h.test/a.png\">",
    "<img src='//h/x' srcset=\"https://h/y 2x\">",
    "<img src=\"http://h/z\" alt=x>",
    "![a](https://h.test/i.png)",
    "<script>",
    "</script>",
    "<a id=\"A\"></a>",
    "[type:\"",
    "|a|b|\n|-|-|\n",
    "<!--",
    "-->",
    "\r\n",
    "\t",
];

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn policy(remote: bool) -> HtmlPolicy {
    let remote = if remote {
        RemoteImagePolicy::AllowHttps
    } else {
        RemoteImagePolicy::Deny
    };
    HtmlPolicy::new(ContentPolicy::try_new(PolicyLimits::default(), remote).unwrap())
}

#[test]
fn random_documents_keep_the_fuzz_invariants() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for iteration in 0..3_000 {
        let flags = (rng.next() & 0xFF) as u8;
        let length = (rng.next() % 64) as usize;
        let input: String = (0..length)
            .map(|_| TOKENS[(rng.next() % TOKENS.len() as u64) as usize])
            .collect();
        let remote = flags & 1 != 0;
        let html = policy(remote);
        if let Ok(clean) = html.sanitize_fragment(&input) {
            let again = html.sanitize_fragment(clean.as_str()).unwrap();
            assert_eq!(
                again.as_str(),
                if clean.as_str().trim().is_empty() {
                    String::new()
                } else {
                    drop_one_lf_after_pre(clean.as_str())
                },
                "#{iteration} sanitizer: {input:?}"
            );
        }
        let limits = RenderLimits {
            max_rendered_html_bytes: if flags & 0x10 != 0 {
                40_000
            } else {
                RenderLimits::DEFAULT_MAX_RENDERED_HTML_BYTES
            },
            max_diagram_count: usize::from((flags >> 5) & 3),
            max_diagram_source_bytes: if flags & 0x80 != 0 { 8 } else { 1024 },
            ..RenderLimits::default()
        };
        let renderer = Renderer::with_engine(marknexia_markdown::PulldownAdapter, limits).unwrap();
        let mut context = RenderContext::new(AppTheme::System, PageIdentity::new([1; 16], [2; 16]));
        context.allow_remote_assets = remote;
        context.enable_diagrams = flags & 2 == 0;
        context.enable_math = flags & 4 == 0;
        if let Ok(document) = renderer.render(&input, &context) {
            let body = document.body().as_str();
            assert!(body.len() <= MAX_HTML_OUTPUT_BYTES);
            assert!(document.page_bytes() <= limits.max_rendered_html_bytes);
            assert_eq!(document.page_html().len(), document.page_bytes());
            assert!(
                !body.to_ascii_lowercase().contains("<script"),
                "#{iteration}: {input:?}"
            );
            if !remote {
                // Text `<` is escaped, so every `<img` here is a real tag.
                for tag in body.split("<img").skip(1) {
                    let tag = tag.split('>').next().unwrap_or_default();
                    assert!(
                        !tag.contains("src=\"http") && !tag.contains("src=\"//"),
                        "#{iteration}: remote image kept: {input:?}\n{body}"
                    );
                }
            }
            let again = policy(remote).sanitize_fragment(body).unwrap();
            // The sanitizer maps white-space-only input to "" by design.
            let expected = if body.trim().is_empty() {
                String::new()
            } else {
                drop_one_lf_after_pre(body)
            };
            if again.as_str() != expected {
                let at = expected
                    .bytes()
                    .zip(again.as_str().bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or(0);
                let from = at.saturating_sub(120);
                panic!(
                    "#{iteration} renderer not a fixed point: {input:?}\n body: {:?}\n again: {:?}",
                    expected.get(from..(at + 80).min(expected.len())),
                    again
                        .as_str()
                        .get(from..(at + 80).min(again.as_str().len()))
                );
            }
        }
    }
}

/// Known sanitizer drift (reported; owned by `marknexia-security`): a `<pre>`
/// whose text starts with a line feed serializes as `<pre>` + LF, and HTML
/// parsing drops a line feed right after `<pre>`, so each re-sanitization
/// removes one. Everything else must be a fixed point.
fn drop_one_lf_after_pre(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<pre") {
        let Some(close) = rest[start..].find('>') else {
            break;
        };
        let end = start + close + 1;
        let is_pre = matches!(rest.as_bytes().get(start + 4), Some(b'>' | b' '));
        out.push_str(&rest[..end]);
        rest = &rest[end..];
        if is_pre && let Some(stripped) = rest.strip_prefix('\n') {
            rest = stripped;
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn pre_leading_line_feed_drift_is_still_present() {
    // Remove this test and `drop_one_lf_after_pre` once the security crate
    // makes `<pre>` serialization a fixed point.
    let policy = HtmlPolicy::new(ContentPolicy::default());
    let once = policy.sanitize_fragment("<pre>\n\nx</pre>").unwrap();
    assert_eq!(once.as_str(), "<pre>\nx</pre>");
    let twice = policy.sanitize_fragment(once.as_str()).unwrap();
    assert_eq!(twice.as_str(), "<pre>x</pre>");
}
