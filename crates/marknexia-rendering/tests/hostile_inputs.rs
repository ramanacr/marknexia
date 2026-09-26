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
/// Generous for unoptimized builds; release runs are far faster.
const CASE_BUDGET: Duration = Duration::from_secs(60);

fn context() -> RenderContext {
    RenderContext::new(AppTheme::System, PageIdentity::new([7; 16], [9; 16]))
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
