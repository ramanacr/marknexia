//! Hostile and limit inputs through the whole renderer: deep nesting on a
//! 1 MiB stack, 4 MiB sources, diagram floods, and exact output-limit edges.
//! Every case must finish within a time bound and either render or fail with
//! a limit error; rendered bodies must be sanitizer-stable and script-free.

use std::time::{Duration, Instant};

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{
    DocumentRenderer, PageIdentity, RenderContext, RenderError, RenderLimits, Renderer,
};
use marknexia_security::{ContentPolicy, HtmlPolicy, SanitizeError};

const ONE_MIB: usize = 1024 * 1024;
/// Generous for unoptimized builds; release runs are far faster. A 4 MiB C#
/// block renders to a 36 MB page, sanitized as one chunk.
const CASE_BUDGET: Duration = Duration::from_secs(if cfg!(debug_assertions) { 300 } else { 60 });

fn context() -> RenderContext {
    RenderContext::new(
        AppTheme::System,
        PageIdentity::new([7; 16], [9; 16]).unwrap(),
    )
}

fn on_one_mib_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(ONE_MIB)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
}

/// Renders `source`, checks the outcome is a success or a limit error, and
/// checks the invariants of a successful render.
fn render_bounded(
    name: &str,
    source: &str,
) -> Result<marknexia_rendering::RenderedDocument, RenderError> {
    let renderer = Renderer::new();
    let started = Instant::now();
    let result = renderer.render(source, &context());
    let elapsed = started.elapsed();
    assert!(elapsed < CASE_BUDGET, "{name}: took {elapsed:?}");
    match &result {
        Ok(document) => {
            let body = document.body().as_str();
            assert!(
                !body.to_ascii_lowercase().contains("<script"),
                "{name}: script in body"
            );
            assert!(document.page_bytes() <= RenderLimits::DEFAULT_MAX_RENDERED_HTML_BYTES);
            let again = HtmlPolicy::new(ContentPolicy::default())
                .sanitize_fragment(body)
                .unwrap_or_else(|error| panic!("{name}: re-sanitize failed: {error}"));
            assert_eq!(again.as_str(), body, "{name}: sanitizer not idempotent");
        }
        Err(
            RenderError::RenderedTooLarge { .. }
            | RenderError::Sanitizer(
                SanitizeError::InputTooLarge { .. }
                | SanitizeError::OutputTooLarge { .. }
                | SanitizeError::NestingTooDeep { .. }
                | SanitizeError::TooComplex,
            ),
        ) => {}
        Err(other) => panic!("{name}: unexpected error {other:?}"),
    }
    eprintln!(
        "{name}: {:?} in {elapsed:?}",
        result
            .as_ref()
            .map(|d| d.page_bytes())
            .map_err(|e| e.to_string())
    );
    result
}

fn nested(open: &str, middle: &str, close: &str, depth: usize) -> String {
    format!("{}{middle}{}", open.repeat(depth), close.repeat(depth))
}

#[test]
fn deep_nesting_on_a_one_mib_stack() {
    on_one_mib_stack(|| {
        let cases = [
            ("quotes", format!("{}x\n", ">".repeat(20_000))),
            ("emphasis", nested("*a ", "x", " b*", 20_000)),
            ("links", nested("[", "x", "](u)", 20_000)),
            ("lists", format!("{}x\n", "- ".repeat(20_000))),
            ("raw-divs", nested("<div>", "x", "</div>", 20_000)),
            ("raw-spans", nested("<span>", "x", "</span>", 50_000)),
            (
                "math-sqrt",
                format!("${}x{}$", "\\sqrt{".repeat(50_000), "}".repeat(50_000)),
            ),
            (
                "math-sup",
                format!("${}x{}$", "^{".repeat(50_000), "}".repeat(50_000)),
            ),
            (
                "quoted-code",
                format!("{}```cs\nvar x = 1;\n```\n", "> ".repeat(5_000)),
            ),
            (
                "quoted-mermaid",
                format!("{}```mermaid\ngraph TD\n```\n", "> ".repeat(5_000)),
            ),
        ];
        for (name, source) in cases {
            let _ = render_bounded(name, &source);
        }
    });
}

#[test]
fn four_mib_inputs_render_or_fail_closed() {
    let four = 4 * ONE_MIB;
    let fill = |unit: &str| unit.repeat((four - 16) / unit.len());
    let cases = [
        ("paragraph", fill("word ")),
        ("lines", fill("line of text\n")),
        (
            "csharp",
            format!("```cs\n{}```\n", fill("var x = \"a\" + 1; // c\n")),
        ),
        (
            "csharp-quotes",
            format!("```cs\n{}\n```\n", fill("\\'\\\"")),
        ),
        ("raw-html", fill("<b>x</b>")),
        ("images", fill("![a](https://h.test/i.png) ")),
        ("math", fill("$a^b$ ")),
        ("entities", fill("&amp;&#x41;&copy;")),
        ("brackets", fill("[type:\"")),
    ];
    for (name, source) in cases {
        assert!(source.len() <= four);
        let _ = render_bounded(name, &source);
    }
}

#[test]
fn small_csharp_blocks_render_and_highlight() {
    let source = "```cs\n/// <summary>x</summary>\n[assembly: A(\"s\")]\n#region r\nvar s = @\"a\"\"b\"; // c\n```\n".repeat(2_000);
    let document = render_bounded("csharp-blocks", &source).expect("renders");
    let body = document.body().as_str();
    assert_eq!(body.matches("class=\"copy-btn\"").count(), 2_000);
    assert_eq!(
        body.matches("<span class=\"stringCSharpVerbatim\">")
            .count(),
        2_000
    );
}

#[test]
fn diagram_flood_is_capped() {
    let source = "```mermaid\ngraph TD\nA-->B\n```\n".repeat(5_000);
    let document = render_bounded("diagram-flood", &source).expect("renders");
    let body = document.body().as_str();
    let shells = body
        .matches("class=\"marknexia-diagram marknexia-mermaid\"")
        .count();
    assert_eq!(shells, RenderLimits::DEFAULT_MAX_DIAGRAM_COUNT);
    assert_eq!(
        body.matches("<p>diagram limit exceeded.</p>").count(),
        5_000 - RenderLimits::DEFAULT_MAX_DIAGRAM_COUNT
    );
    assert!(document.has_mermaid());
    assert!(document.page_html().contains("mermaid.min.js"));
    assert_eq!(document.diagrams().len(), 5_000);
    assert_eq!(
        document.diagnostics().len(),
        5_000 - RenderLimits::DEFAULT_MAX_DIAGRAM_COUNT
    );
}

#[test]
fn diagram_source_limit_edges() {
    let source = "```mermaid\nabcd\n```\n";
    // Decoded source is "abcd\n", five bytes.
    for (limit, rendered) in [(5, true), (4, false)] {
        let limits = RenderLimits {
            max_diagram_source_bytes: limit,
            ..RenderLimits::default()
        };
        let renderer = Renderer::with_engine(marknexia_markdown::PulldownAdapter, limits).unwrap();
        let document = renderer.render(source, &context()).unwrap();
        assert_eq!(document.has_mermaid(), rendered, "limit {limit}");
        assert_eq!(
            document
                .body()
                .as_str()
                .contains("diagram source limit exceeded"),
            !rendered
        );
    }
    let mut disabled = context();
    disabled.enable_diagrams = false;
    let document = Renderer::new().render(source, &disabled).unwrap();
    assert!(!document.has_mermaid());
    assert!(!document.page_html().contains("mermaid.min.js"));
    assert!(
        document
            .body()
            .as_str()
            .contains("diagram rendering is disabled")
    );
}

#[test]
fn output_limit_edges_are_exact() {
    let source = "# bounded output\n\n```cs\nvar x = 1;\n```\n";
    let exact = Renderer::new()
        .render(source, &context())
        .unwrap()
        .page_bytes();
    for (limit, ok) in [(exact, true), (exact - 1, false), (1, false)] {
        let limits = RenderLimits {
            max_rendered_html_bytes: limit,
            ..RenderLimits::default()
        };
        let renderer = Renderer::with_engine(marknexia_markdown::PulldownAdapter, limits).unwrap();
        match renderer.render(source, &context()) {
            Ok(document) => {
                assert!(ok, "limit {limit} should reject");
                assert_eq!(document.page_html().len(), exact);
            }
            Err(RenderError::RenderedTooLarge {
                size_bytes,
                maximum_bytes,
            }) => {
                assert!(!ok, "limit {limit} should render");
                assert_eq!(maximum_bytes, limit as u64);
                // The Markdown layer stops early when even the body is over.
                if let Some(size) = size_bytes {
                    assert_eq!(size, exact as u64);
                }
            }
            Err(other) => panic!("{other:?}"),
        }
    }
    assert_eq!(
        Renderer::with_engine(
            marknexia_markdown::PulldownAdapter,
            RenderLimits {
                max_rendered_html_bytes: 0,
                ..RenderLimits::default()
            }
        )
        .unwrap_err(),
        RenderError::InvalidLimits
    );
}

#[test]
fn source_limit_edges_are_exact() {
    let renderer = Renderer::new();
    let max = RenderLimits::DEFAULT_MAX_SOURCE_BYTES;
    assert_eq!(renderer.check_source_size(max), Ok(()));
    assert_eq!(
        renderer.check_source_size(max + 1),
        Err(RenderError::SourceTooLarge {
            size_bytes: max + 1,
            maximum_bytes: max
        })
    );
    let limits = RenderLimits {
        max_source_bytes: 4,
        ..RenderLimits::default()
    };
    let small = Renderer::with_engine(marknexia_markdown::PulldownAdapter, limits).unwrap();
    assert!(small.render("abcd", &context()).is_ok());
    assert_eq!(
        small.render("abcde", &context()).unwrap_err(),
        RenderError::SourceTooLarge {
            size_bytes: 5,
            maximum_bytes: 4
        }
    );
}

#[test]
fn metadata_is_plain_text_and_remote_images_are_blanked() {
    let source = "# a < b & \"c\" <script>alert(1)</script>\n\n<a id=\"X&quot;\"></a>\n\n![r](https://h.test/a.png) ![l](local.png)\n";
    let document = Renderer::new().render(source, &context()).unwrap();
    assert_eq!(
        document.headings()[0].text.as_str(),
        "a < b & \"c\" alert(1)"
    );
    let encoded = document.headings()[0]
        .text
        .encode_html(&HtmlPolicy::new(ContentPolicy::default()))
        .unwrap();
    assert_eq!(encoded.as_str(), "a &lt; b &amp; &quot;c&quot; alert(1)");
    let body = document.body().as_str();
    assert!(body.contains("<img src=\"\" alt=\"r\">"), "{body}");
    assert!(body.contains("<img src=\"local.png\" alt=\"l\">"), "{body}");
    let images: Vec<_> = document
        .image_references()
        .iter()
        .map(|i| i.as_str())
        .collect();
    assert_eq!(images, ["https://h.test/a.png", "local.png"]);
    assert!(
        document
            .anchors()
            .iter()
            .any(|anchor| anchor.is_heading_anchor)
    );
}

/// Where the time goes for the slowest 4 MiB shapes (run with `--ignored`).
#[test]
#[ignore = "timing report"]
fn stage_timing() {
    use marknexia_markdown::{MarkdownEngine, MarkdownOptions, PulldownAdapter};
    for (name, unit) in [
        ("raw-html", "<b>x</b>"),
        ("paragraph", "word "),
        ("fixture-code", "```c#\nvar value = 1;\n```\n"),
    ] {
        let source = unit.repeat((4 * ONE_MIB - 16) / unit.len());
        let started = Instant::now();
        let parsed = PulldownAdapter
            .parse(&source, &MarkdownOptions::default())
            .unwrap();
        let parse = started.elapsed();
        let started = Instant::now();
        let sanitized =
            HtmlPolicy::new(ContentPolicy::default()).sanitize_fragment(&parsed.rendered_body_html);
        let sanitize = started.elapsed();
        let started = Instant::now();
        let rendered = Renderer::new().render(&source, &context());
        let total = started.elapsed();
        eprintln!(
            "{name}: source {} B, parse {parse:?}, sanitize-only {sanitize:?} ({}), full render {total:?} ({})",
            source.len(),
            sanitized.is_ok(),
            rendered
                .map(|d| d.page_bytes().to_string())
                .unwrap_or_else(|e| e.to_string())
        );
    }
}

#[test]
fn csharp_attribute_prefix_floods_stay_linear() {
    // Review C1: every `[type:` used to rescan to the end for `]`/`"`
    // (release: n=16,000 took 1.16 s, n=64,000 took 27.2 s). The sizes stay
    // inside the sanitizer budget so the tokenizer really runs.
    for n in [16_000, 64_000, 200_000] {
        for (name, source) in [
            ("prefixes", format!("```cs\n{}\n```\n", "[type:".repeat(n))),
            (
                "prefixes-quote",
                format!("```cs\n{}\"\n```\n", "[type:".repeat(n)),
            ),
            (
                "prefixes-string",
                format!("```cs\n{}\"x\"\n```\n", "[type:".repeat(n)),
            ),
            (
                "quoted-prefixes",
                format!("```cs\n{}]\n```\n", "[type:\"x".repeat(n / 2)),
            ),
        ] {
            let started = Instant::now();
            let document = render_bounded(&format!("{name} n={n}"), &source)
                .unwrap_or_else(|error| panic!("{name} n={n}: {error}"));
            assert!(document.body().as_str().contains("class=\"csharp\""));
            // The quadratic regression took 27 s at n=64,000 in release; debug
            // builds on busy or 4-core CI hosts need headroom, not a tight bound.
            let bound = Duration::from_secs(if cfg!(debug_assertions) { 120 } else { 10 });
            assert!(started.elapsed() < bound, "{name} n={n}");
        }
    }
}

#[test]
fn amplifying_blocks_fail_before_building_huge_strings() {
    // `1 ` highlights to ~30 bytes per 2 input bytes; the pass must reject
    // against the 128 MiB budget instead of building far past it first.
    let digits = format!("```cs\n{}\n```\n", "1 ".repeat(4_600_000));
    assert!(matches!(
        render_bounded("digits", &digits),
        Err(RenderError::Sanitizer(SanitizeError::InputTooLarge { .. }))
    ));
    let fractions = format!("${}$", r"\frac{a}{b}".repeat(1_300_000));
    assert!(matches!(
        render_bounded("fractions", &fractions),
        Err(RenderError::Sanitizer(SanitizeError::InputTooLarge { .. }))
    ));
}

#[test]
fn deep_math_inside_deep_markdown_stays_within_sanitizer_depth() {
    // Review I3: maximal Markdown nesting plus maximal math nesting must not
    // reach the sanitizer's 256-level limit.
    on_one_mib_stack(|| {
        let math = format!("${}x{}$", r"\frac{\sqrt{".repeat(200), "}}".repeat(200));
        let inline = format!("{}{math}{}", "*a ".repeat(40), " b*".repeat(40));
        let source = format!("{}{inline}\n", "> - ".repeat(40));
        let document = render_bounded("deep-math-in-markdown", &source).expect("renders");
        assert!(document.body().as_str().contains("marknexia-fraction"));
    });
}

#[test]
fn remote_image_sources_are_blanked_like_dotnet() {
    // REND-4: one sanitizer pass under `RemoteImagePolicy::Blank` leaves
    // what .NET's sanitize-then-`BlockRemoteImages` leaves, for `https://`,
    // `http://` and `//` alike.
    for (source, expected) in [
        (
            "![r](https://h.test/a.png)",
            "<p><img src=\"\" alt=\"r\"></p>\n",
        ),
        (
            "![r](http://h.test/a.png)",
            "<p><img src=\"\" alt=\"r\"></p>\n",
        ),
        (
            "<img src=\"//h.test/a.png\" alt=\"p\">",
            "<img src=\"\" alt=\"p\">
",
        ),
        (
            "![l](local.png)",
            "<p><img src=\"local.png\" alt=\"l\"></p>\n",
        ),
    ] {
        let document = Renderer::new().render(source, &context()).unwrap();
        assert_eq!(document.body().as_str(), expected, "{source}");
    }
}

#[test]
fn remote_sources_dotnet_leaks_are_removed() {
    // REND-4: .NET blanks only quoted `<img src>` values without a quote
    // character inside, so these keep their remote URL there (only its CSP
    // blocks the fetch). Rust removes them.
    for source in [
        "<img src=\"https://h.test/a'b.png\" alt=\"q\">",
        "<input type=\"image\" src=\"https://h.test/i.png\">",
    ] {
        let document = Renderer::new().render(source, &context()).unwrap();
        let body = document.body().as_str();
        assert!(!body.contains("h.test"), "{source} -> {body}");
    }
}

/// Covers only the sanitized page the host serves, before any runtime runs:
/// source markup stays text and the runtime tag comes only from the
/// template. What the Mermaid runtime does with click bindings and init
/// directives is checked natively in `marknexia-webview`
/// `session::native_tests::native_mermaid_runtime_renders_and_falls_back`.
#[test]
fn hostile_sources_stay_inert_in_the_served_body() {
    const RUNTIME_TAG: &str = "<script src=\"https://marknexia.assets/mermaid.min.js\"";
    let cases = [
        (
            "body-mermaid-breakout",
            "```mermaid\n</pre></div></div><script>alert(1)</script><img src=x onerror=alert(2)>\n```\n",
            true,
        ),
        (
            "body-mermaid-runtime-tag-in-source",
            "```mermaid\n<script src=\"https://marknexia.assets/mermaid.min.js\"></script>\n```\n",
            true,
        ),
        (
            "body-mermaid-click-callback",
            "```mermaid\ngraph TD\nA-->B\nclick A callback \"javascript:alert(1)\"\n```\n",
            true,
        ),
        (
            "body-mermaid-init-directive",
            "```mermaid\n%%{init: {\"securityLevel\": \"loose\"}}%%\ngraph TD\nA[\"<img src=x onerror=alert(1)>\"]\n```\n",
            true,
        ),
        (
            "raw-runtime-tag",
            "<script src=\"https://marknexia.assets/mermaid.min.js\"></script>\n\n<div class=\"marknexia-diagram marknexia-mermaid\" id=\"mermaid-1\"><pre class=\"mermaid\">graph TD</pre></div>\n",
            false,
        ),
        (
            "math-text-markup",
            "$\\text{<img src=x onerror=alert(1)>}$ and $\\operatorname{</span><script>x</script>}$\n",
            false,
        ),
        (
            "math-raw-block-breakout",
            "<div class=\"math\">$$</div><script>alert(1)</script>$$</div>\n",
            false,
        ),
        (
            "math-delimiters",
            "$\\left< a \\right> \\left& b \\right\"$ and $a^\\\"$\n",
            false,
        ),
    ];
    for (name, source, has_mermaid) in cases {
        let document = render_bounded(name, source).expect("renders");
        let body = document.body().as_str().to_ascii_lowercase();
        // Markup in a source stays text: no live attribute survives.
        assert!(!body.contains(" onerror=\""), "{name}: {body}");
        assert!(!body.contains("=\"javascript:"), "{name}: {body}");
        assert_eq!(document.has_mermaid(), has_mermaid, "{name}");
        let page = document.page_html();
        assert_eq!(
            page.matches(RUNTIME_TAG).count(),
            usize::from(has_mermaid),
            "{name}: the runtime tag comes only from the template"
        );
    }
}

#[test]
fn math_expansion_floods_render_within_the_page_limit() {
    // `\frac` and `\sqrt` grow about tenfold in the math pass (REND-1).
    let four = 4 * ONE_MIB;
    for (name, unit) in [
        ("math-frac-flood", "$\\frac{a}{b}$ "),
        ("math-sqrt-flood", "$\\sqrt[3]{x}$ "),
        ("math-display-flood", "$$\\frac{\\alpha}{\\beta}$$ "),
    ] {
        let count = (four - 16) / unit.len();
        let source = unit.repeat(count);
        // Each flood fits: about 35 to 60 MB of page, under the 128 MiB limit.
        let document = render_bounded(name, &source)
            .unwrap_or_else(|error| panic!("{name}: expected a rendered page, got {error:?}"));
        let page_bytes = document.page_bytes();
        assert!(
            (16 * ONE_MIB..=RenderLimits::DEFAULT_MAX_RENDERED_HTML_BYTES).contains(&page_bytes),
            "{name}: {page_bytes}"
        );
        assert_eq!(
            document
                .body()
                .as_str()
                .matches("class=\"marknexia-math\" role=\"math\"")
                .count(),
            count,
            "{name}: every span rendered"
        );
    }
}

/// A fence name for every ColorCode language (the Markdown info string
/// allows only `[a-zA-Z0-9_+#-]`, so ids such as `vb.net` use an alias).
const LANGUAGE_FENCES: &[(&str, &str)] = &[
    ("javascript", "javascript"),
    ("html", "html"),
    ("c#", "cs"),
    ("vb.net", "vb"),
    ("ashx", "ashx"),
    ("asax", "asax"),
    ("aspx", "aspx"),
    ("aspx(c#)", "aspx-cs"),
    ("aspx(vb.net)", "aspx-vb"),
    ("sql", "sql"),
    ("xml", "xml"),
    ("php", "php"),
    ("css", "css"),
    ("cpp", "cpp"),
    ("java", "java"),
    ("json", "json"),
    ("powershell", "powershell"),
    ("typescript", "typescript"),
    ("f#", "fs"),
    ("koka", "koka"),
    ("haskell", "haskell"),
    ("markdown", "markdown"),
    ("fortran", "fortran"),
    ("python", "python"),
    ("matlab", "matlab"),
];

fn highlight_oracle_code(name: &str) -> String {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("oracle/highlight.v1.json")).unwrap();
    oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap_or_else(|| panic!("{name}"))["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn fenced(fence: &str, code: &str) -> String {
    format!("~~~~~~~~ {fence}\n{code}\n~~~~~~~~\n\n")
}

/// Pathological shapes for every language: the oracle's token soups and
/// unterminated quotes, scaled up. Each block highlights or falls back to
/// plain text within the regex budget (HL-1); the render stays bounded.
#[test]
fn pathological_blocks_in_every_language_fail_closed() {
    for &(id, fence) in LANGUAGE_FENCES {
        let mut source = String::new();
        for shape in ["soup-2", "soup-3", "soup-5", "long-unterminated-quotes"] {
            let code = highlight_oracle_code(&format!("{id}/{shape}"));
            source.push_str(&fenced(fence, &code.repeat(8)));
        }
        let started = Instant::now();
        let document = render_bounded(&format!("pathological {id}"), &source).expect("renders");
        let body = document.body().as_str();
        assert_eq!(body.matches("class=\"code-container\"").count(), 4, "{id}");
        // One document budget: about a second of matching plus the per-unit
        // credit, whatever the number of hostile blocks.
        let bound = Duration::from_secs(if cfg!(debug_assertions) { 120 } else { 10 });
        assert!(started.elapsed() < bound, "{id}: {:?}", started.elapsed());
    }
}

/// Many catastrophic blocks share one budget, so the document cannot take
/// one budget per block (HL-1).
#[test]
fn catastrophic_blocks_share_one_document_budget() {
    let code = highlight_oracle_code("json/soup-3");
    let source = fenced("json", &code).repeat(300);
    let started = Instant::now();
    let document = render_bounded("json soups", &source).expect("renders");
    let elapsed = started.elapsed();
    let body = document.body().as_str();
    assert_eq!(body.matches("class=\"code-container\"").count(), 300);
    assert_eq!(body.matches("<div class=\"json\">").count(), 0);
    assert_eq!(body.matches("<pre><code>").count(), 300);
    let bound = Duration::from_secs(if cfg!(debug_assertions) { 120 } else { 10 });
    assert!(elapsed < bound, "{elapsed:?}");
}

/// Large ordinary blocks highlight in every language and stay linear.
#[test]
fn large_ordinary_blocks_highlight_in_every_language() {
    for &(id, fence) in LANGUAGE_FENCES {
        let line = highlight_oracle_code(&format!("{id}/long-line"));
        // About 1.3 MB of code per block.
        let source = fenced(fence, &line.repeat(240));
        let document = render_bounded(&format!("large {id}"), &source);
        let Ok(document) = document else {
            continue;
        };
        // Highlighted, not the plain `<pre><code>` fallback.
        assert!(
            !document.body().as_str().contains("<pre><code>"),
            "{id}: not highlighted"
        );
    }
}
