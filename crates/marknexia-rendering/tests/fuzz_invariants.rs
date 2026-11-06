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
    "\0",
    "\u{feff}",
    "\u{1}",
    "\u{b}",
    "\u{c}",
    // Phase 3: Mermaid runtime and math inputs.
    "<script src=\"https://marknexia.assets/mermaid.min.js\" nonce=\"x\"></script>",
    "<div class=\"marknexia-diagram marknexia-mermaid\" id=\"mermaid-1\">",
    "<div class=\"math\">",
    "$$\n",
    "\\left(",
    "\\right.",
    "\\text{",
    "\\alpha",
    "\u{1d465}",
    "graph TD\nA-->B\n",
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

/// The renderer's policy: remote images kept (remote assets on) or blanked.
fn policy(remote: bool) -> HtmlPolicy {
    policy_for(if remote {
        RemoteImagePolicy::AllowHttps
    } else {
        RemoteImagePolicy::Blank
    })
}

fn policy_for(remote: RemoteImagePolicy) -> HtmlPolicy {
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
        let html = if flags & 8 != 0 {
            policy_for(RemoteImagePolicy::Deny)
        } else {
            policy(remote)
        };
        if let Ok(clean) = html.sanitize_fragment(&input) {
            // Sanitizing is a fixed point, including white space and line
            // feeds after `<pre>`/`<textarea>` (I2, M1).
            let again = html.sanitize_fragment(clean.as_str()).unwrap();
            assert_eq!(again, clean, "#{iteration} sanitizer: {input:?}");
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
        let mut context = RenderContext::new(
            AppTheme::System,
            PageIdentity::new([1; 16], [2; 16]).unwrap(),
        );
        context.allow_remote_assets = remote;
        context.enable_diagrams = flags & 2 == 0;
        context.enable_math = flags & 4 == 0;
        if let Ok(document) = renderer.render(&input, &context) {
            let body = document.body().as_str();
            assert!(body.len() <= MAX_HTML_OUTPUT_BYTES);
            assert!(document.page_bytes() <= limits.max_rendered_html_bytes);
            let page = document.page_html();
            assert_eq!(page.len(), document.page_bytes());
            // The page loads the Mermaid runtime once, after the body, exactly
            // when a diagram shell was emitted; raw HTML cannot add it.
            const RUNTIME_TAG: &str = "<script src=\"https://marknexia.assets/mermaid.min.js\"";
            const BODY_OPEN: &str = "<div class=\"markdown-body\">\n";
            let tags = page.matches(RUNTIME_TAG).count();
            assert_eq!(
                tags,
                usize::from(document.has_mermaid()),
                "#{iteration}: {input:?}"
            );
            if tags == 1 {
                let body_end = page.find(BODY_OPEN).unwrap() + BODY_OPEN.len() + body.len();
                assert!(page.find(RUNTIME_TAG).is_some_and(|tag| tag >= body_end));
            }
            if !context.enable_diagrams || limits.max_diagram_count == 0 {
                assert!(!document.has_mermaid(), "#{iteration}: {input:?}");
            }
            // A shell can still vanish in sanitizing (inside raw `<script>`
            // or a comment), so `has_mermaid` does not imply a visible shell;
            // .NET decides `hasMermaid` before sanitizing too.
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
            let expected = body;
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

#[test]
fn leading_line_feeds_in_pre_and_textarea_are_a_fixed_point() {
    // I2: the parser drops one line feed after `<pre>`/`<textarea>`; the
    // serializer writes it back when the text starts with a line feed.
    let policy = HtmlPolicy::new(ContentPolicy::default());
    for (input, expected) in [
        ("<pre>\n\nx</pre>", "<pre>\n\nx</pre>"),
        ("<pre>\nx</pre>", "<pre>x</pre>"),
        ("<pre>\n\n\nx</pre>", "<pre>\n\n\nx</pre>"),
        ("<textarea>\n\nx</textarea>", "<textarea>\n\nx</textarea>"),
        ("<pre><x-foo>\nabc</x-foo></pre>", "<pre>\n\nabc</pre>"),
        ("<pre><!--c-->\nabc</pre>", "<pre>\n\nabc</pre>"),
        ("<pre><b>\nx</b></pre>", "<pre><b>\nx</b></pre>"),
    ] {
        let once = policy.sanitize_fragment(input).unwrap();
        assert_eq!(once.as_str(), expected, "{input:?}");
        let twice = policy.sanitize_fragment(once.as_str()).unwrap();
        assert_eq!(twice, once, "{input:?}");
    }
}
