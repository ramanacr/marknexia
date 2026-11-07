//! Text diagrams (Rust edition, `compat/decisions/text-diagrams.md`): the
//! detection corpus, the rendered shell after sanitizing, the setting, the
//! limits and hostile inputs.

use std::time::{Duration, Instant};

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{
    DocumentRenderer, PageIdentity, RenderContext, RenderError, RenderedDocument, Renderer,
    testing::{TextDiagramDetection, detect_text_diagram},
};
use marknexia_security::{ContentPolicy, HtmlPolicy};

/// The "Reference architecture" block of
/// `smart-token-reducer/Implementation Docs/02_ARCHITECTURE_ADRS.md`.
const ADR: &str = include_str!("text_diagrams/adr-reference-architecture.txt");

const CASE_BUDGET: Duration = Duration::from_secs(if cfg!(debug_assertions) { 60 } else { 10 });

fn context() -> RenderContext {
    RenderContext::new(
        AppTheme::System,
        PageIdentity::new([7; 16], [9; 16]).unwrap(),
    )
}

fn render(source: &str) -> RenderedDocument {
    render_with(source, &context()).unwrap()
}

fn render_with(source: &str, context: &RenderContext) -> Result<RenderedDocument, RenderError> {
    let started = Instant::now();
    let result = Renderer::new().render(source, context);
    assert!(
        started.elapsed() < CASE_BUDGET,
        "took {:?}",
        started.elapsed()
    );
    if let Ok(document) = &result {
        let body = document.body().as_str();
        // The body is sanitizer-stable and script-free.
        let again = HtmlPolicy::new(ContentPolicy::default())
            .sanitize_fragment(body)
            .unwrap();
        assert_eq!(again.as_str(), body);
        // Text `<` is always encoded, so every `<` starts a real tag.
        for tag in body.split('<').skip(1) {
            let tag = tag.split('>').next().unwrap_or("").to_ascii_lowercase();
            let name = tag.split([' ', '/']).next().unwrap_or("");
            assert!(
                !matches!(name, "script" | "style" | "foreignobject" | "use" | "image"),
                "<{tag}> in body"
            );
            for (attribute, value) in attributes(&tag[name.len()..]) {
                assert!(!attribute.starts_with("on"), "event handler in <{tag}>");
                assert!(
                    !value.contains("javascript:")
                        || attribute.starts_with("aria-")
                        || attribute == "data-copy-text",
                    "script URL in <{tag}>"
                );
            }
        }
    }
    result
}

/// The attributes of a serialized tag (double-quoted values only, as the
/// sanitizer writes them).
fn attributes(mut rest: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    loop {
        rest = rest.trim_start_matches([' ', '\n', '/']);
        if rest.is_empty() {
            return found;
        }
        let name_end = rest.find(['=', ' ']).unwrap_or(rest.len());
        let name = &rest[..name_end];
        rest = &rest[name_end..];
        let mut value = "";
        if let Some(quoted) = rest.strip_prefix("=\"") {
            let end = quoted.find('"').unwrap_or(quoted.len());
            value = &quoted[..end];
            rest = quoted.get(end + 1..).unwrap_or("");
        }
        found.push((name, value));
    }
}

fn fenced(info: &str, code: &str) -> String {
    format!("```{info}\n{code}\n```\n")
}

// ---------------------------------------------------------------------------
// Detection corpus.
// ---------------------------------------------------------------------------

const POSITIVE: &[(&str, &str)] = &[
    ("adr reference architecture", ADR),
    (
        "box drawing with arrows",
        "┌────────┐     ┌─────────┐
│ Client │────▶│ Server  │
└────────┘     └────┬────┘
                    │
                    ▼
               ┌─────────┐
               │   DB    │
               └─────────┘",
    ),
    (
        "ascii boxes with arrows",
        "+--------+      +--------+
| parser | ---> | render |
+--------+      +--------+
     |               |
     v               v
  [tokens]        [html]",
    ),
    (
        "unicode arrow flow",
        "request → router → handler
             │
             ↓
          storage",
    ),
    (
        "sequence diagram",
        "Client          Server
  |   request    |
  |------------->|
  |   response   |
  |<-------------|",
    ),
    (
        "rounded ascii boxes",
        ".-------.     .-------.
| start | --> |  end  |
'-------'     '-------'
    |
    v
  done",
    ),
    (
        "box tree with arrows",
        "app
├── ui ──→ renders pages
├── core ──→ owns state
└── io ──→ files and network",
    ),
    (
        "pipeline with branches",
        "input
  |
  +--> lint --> report
  |
  +--> build --> artifact
  |
  v
deploy",
    ),
];

const NEGATIVE: &[(&str, &str)] = &[
    (
        "rust",
        "fn main() -> Result<(), Error> {
    let x = compute(1, 2);
    println!(\"{x}\");
    Ok(())
}",
    ),
    (
        "haskell signatures",
        "map :: (a -> b) -> [a] -> [b]
map _ [] = []
map f (x:xs) = f x : map f xs
compose :: (b -> c) -> (a -> b) -> a -> c",
    ),
    (
        "c pointers",
        "node->next = head;
head->prev = node;
head = node;
if (node->next) { count++; }",
    ),
    (
        "python type hints",
        "def parse(text: str) -> Tree:
    tokens = lex(text)
    return build(tokens)

def lex(text: str) -> list[str]:
    return text.split()",
    ),
    (
        "json",
        "{
  \"name\": \"marknexia\",
  \"arrows\": [\"->\", \"-->\"],
  \"nested\": { \"a\": 1 }
}",
    ),
    (
        "yaml",
        "steps:
  - name: build
    run: cargo build
  - name: test
    run: cargo test",
    ),
    (
        "shell session",
        "$ cargo build
   Compiling marknexia v0.1.0
    Finished dev [unoptimized] target(s)
$ ls -> out.txt",
    ),
    (
        "powershell session",
        "PS C:\\repo> git status
On branch main
PS C:\\repo> git log --oneline -1
9f4f145 release: v1.0.18",
    ),
    (
        "log with arrows",
        "2026-11-07 12:00:01 INFO state idle -> loading
2026-11-07 12:00:02 INFO state loading -> ready
2026-11-07 12:00:03 WARN retry |---> 2
2026-11-07 12:00:04 INFO done",
    ),
    (
        "rustc diagnostic",
        "error[E0308]: mismatched types
 --> src/main.rs:2:5
  |
2 |     x
  |     ^ expected `u32`, found `&str`
  |
  = note: expected type",
    ),
    (
        "psql output",
        " id | name  | role
----+-------+------
  1 | alice | admin
  2 | bob   | user
(2 rows)",
    ),
    (
        "markdown table",
        "| Column | Value |
|--------|-------|
| a      | 1     |
| b      | 2     |",
    ),
    (
        "ascii table",
        "+------+-------+
| name | count |
+------+-------+
| a    |     1 |
| b    |     2 |
+------+-------+",
    ),
    (
        "git log graph",
        "* 9f4f145 release: v1.0.18
*   e004f30 merge: apply theme
|\\
| * 217b86b fix: preserve defaults
| * 5ca65b8 fix: scope states
|/
* fd4f761 fix: complete states",
    ),
    (
        "tree listing",
        "marknexia
├── crates
│   ├── marknexia-core
│   └── marknexia-rendering
├── docs
└── scripts",
    ),
    (
        "cargo tree",
        "marknexia-rendering v0.1.0
├── marknexia-core v0.1.0
│   ├── serde v1.0.229
│   └── serde_json v1.0.149
└── markup5ever v0.40.0
    └── tendril v0.5.1",
    ),
    (
        "unified diff",
        "diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
-old --> line
+new --> line
 |   context",
    ),
    (
        "ls -l with symlinks",
        "total 8
lrwxrwxrwx 1 u g 9 Nov 7 current -> v1.0.18
lrwxrwxrwx 1 u g 9 Nov 7 latest -> v1.0.18
drwxr-xr-x 2 u g 4096 Nov 7 v1.0.18",
    ),
    (
        "sql",
        "SELECT id, name
FROM users
WHERE id > 1
ORDER BY name;",
    ),
    (
        "python traceback",
        "Traceback (most recent call last):
  File \"main.py\", line 3, in <module>
    run()
  |
  v
ValueError: bad",
    ),
    (
        "prose",
        "This block holds plain prose in a code fence.
It mentions a -> b once and uses a | pipe.
Nothing here is a drawing.",
    ),
    (
        "arrow changelog",
        "v1 -> v2: renamed the setting
v2 -> v3: removed the flag
v3 -> v4: added the menu item",
    ),
    (
        "elixir pipes",
        "input
|> parse()
|> validate()
|> render()",
    ),
    (
        "terraform plan",
        "  ~ resource \"aws_instance\" \"web\" {
      ~ ami = \"ami-1\" -> \"ami-2\"
        id  = \"i-1\"
    }",
    ),
    (
        "http request",
        "GET /index.html HTTP/1.1
Host: example.test
Accept: text/html",
    ),
    (
        "progress output",
        "[=====>      ] 45% downloading
[=========>  ] 80% downloading
[===========>] 100% done",
    ),
    (
        "csv",
        "name,value,note
a,1,x->y
b,2,y->z",
    ),
    ("single arrow line", "Settings → Privacy → Camera"),
];

fn report(name: &str, detection: &TextDiagramDetection) -> String {
    format!(
        "{name:<28} accepted={:<5} rows={}/{} drawn={}/{} heads={} box={} vertical={}",
        detection.accepted,
        detection.diagram_rows,
        detection.nonblank_rows,
        detection.drawn_cells,
        detection.nonspace_cells,
        detection.heads,
        detection.box_cells,
        detection.vertical_cells
    )
}

#[test]
fn corpus_positive_blocks_are_diagrams() {
    let mut failures = Vec::new();
    for (name, code) in POSITIVE {
        let detection = detect_text_diagram(code);
        eprintln!("+ {}", report(name, &detection));
        if !detection.accepted {
            failures.push(*name);
        }
    }
    assert!(failures.is_empty(), "rejected diagrams: {failures:?}");
}

#[test]
fn corpus_negative_blocks_stay_text() {
    let mut failures = Vec::new();
    for (name, code) in NEGATIVE {
        let detection = detect_text_diagram(code);
        eprintln!("- {}", report(name, &detection));
        if detection.accepted {
            failures.push(*name);
        }
        // Untagged and `text` fences render exactly as before.
        for info in ["", "text"] {
            let document = render(&fenced(info, code));
            assert_eq!(document.text_diagrams(), 0, "{name} ({info:?})");
            assert!(
                document
                    .body()
                    .as_str()
                    .contains("class=\"code-container\"")
            );
        }
    }
    assert!(failures.is_empty(), "accepted non-diagrams: {failures:?}");
}

// ---------------------------------------------------------------------------
// Rendering.
// ---------------------------------------------------------------------------

#[test]
fn adr_block_renders_as_a_sanitized_diagram() {
    // `fenced` adds a line feed after the file's own: trailing blank lines
    // are not part of the grid (26 rows, 52ch).
    let source = format!("## Reference architecture\n{}\n## Next\n", fenced("", ADR));
    let document = render(&source);
    assert_eq!(document.text_diagrams(), 1);
    let body = document.body().as_str();
    // The shell, the toolbar the bridge drives, and the accessible text.
    for expected in [
        "<div class=\"marknexia-diagram marknexia-mermaid marknexia-text-diagram\" id=\"marknexia-text-diagram-1\">",
        "<span class=\"marknexia-diagram-label\">Diagram</span>",
        "data-marknexia-action=\"copy\" data-copy-text=\"Claude%20Code%20session",
        "data-marknexia-action=\"toggle-source\"",
        "data-marknexia-action=\"zoom-in\"",
        "data-marknexia-action=\"expand\"",
        "id=\"marknexia-text-diagram-1-viewport\"",
        "<div class=\"marknexia-text-diagram-figure\" role=\"img\" aria-label=\"Claude Code session",
        "<svg class=\"marknexia-text-diagram-svg\" viewBox=\"0 0 ",
        "<path class=\"tdg-line\" d=\"M",
        "<path class=\"tdg-head\" d=\"M",
        "<div class=\"marknexia-text-diagram-text\" aria-hidden=\"true\">Claude Code session",
        "id=\"marknexia-text-diagram-1-source\" style=\"display: none\"",
        "<pre><code>Claude Code session",
    ] {
        assert!(body.contains(expected), "missing {expected}");
    }
    // The figure keeps its grid size through the CSS sanitizer.
    assert!(
        body.contains("style=\"width: 108ch; height: 52ch\""),
        "{}",
        figure_style(body)
    );
    // Text that is not a connector stays text; drawn cells are blanked.
    assert!(body.contains("short-lived process per call"));
    assert!(body.contains("Pre|PostCompact"));
    assert!(body.contains("[--lines|--grep|--tail|--head]"));
    let layer_start = body
        .find("<div class=\"marknexia-text-diagram-text\"")
        .unwrap();
    let layer = &body[layer_start..layer_start + body[layer_start..].find("</div>").unwrap()];
    assert!(layer.contains("    metrics event writer"));
    assert!(!layer.contains("+--&gt;"));
    for text in [
        "Pre|PostCompact",
        "[--lines|--grep|--tail|--head]",
        "short-lived",
    ] {
        assert!(layer.contains(text), "{text} in the text layer");
    }
    // The accessible name and the source keep the original text.
    assert_eq!(body.matches("+--&gt; metrics").count(), 2);
    // The page carries the diagram stylesheet; no Mermaid runtime.
    assert!(!document.has_mermaid());
    let page = document.page_html();
    assert!(page.contains("/* Text diagrams (Rust edition, TDG-7) */"));
    assert!(!page.contains("mermaid.min.js"));
}

fn figure_style(body: &str) -> &str {
    let start = body
        .find("ch; height")
        .map_or(0, |at| at.saturating_sub(40));
    &body[start..(start + 200).min(body.len())]
}

#[test]
fn tagged_fences_always_render() {
    let table = "+---+---+\n| a | b |\n+---+---+";
    assert!(!detect_text_diagram(table).accepted);
    for tag in ["bob", "svgbob", "ascii", "diagram", "Diagram"] {
        let document = render(&fenced(tag, table));
        assert_eq!(document.text_diagrams(), 1, "{tag}");
    }
    for tag in ["", "text", "txt", "rust"] {
        let document = render(&fenced(tag, table));
        assert_eq!(document.text_diagrams(), 0, "{tag}");
    }
}

#[test]
fn the_setting_turns_conversion_off_and_restores_the_dotnet_page() {
    let source = format!("# A\n{}", fenced("bob", "a --> b\n|\nv\nc"));
    let mut off = context();
    off.enable_text_diagrams = false;
    let document = render_with(&source, &off).unwrap();
    assert_eq!(document.text_diagrams(), 0);
    assert!(
        document
            .body()
            .as_str()
            .contains("class=\"code-container\"")
    );
    assert!(!document.page_html().contains("Text diagrams (Rust edition"));
    // A document without diagrams has the same page with the setting on.
    let plain = "# A\n```rust\nlet a = 1;\n```\n";
    assert_eq!(
        render_with(plain, &off).unwrap().page_html(),
        render(plain).page_html()
    );
}

#[test]
fn mermaid_and_text_diagrams_coexist() {
    let source = format!(
        "{}{}",
        fenced("mermaid", "graph TD\nA-->B"),
        fenced("diagram", "a --> b\n|\nv")
    );
    let document = render(&source);
    assert!(document.has_mermaid());
    assert_eq!(document.text_diagrams(), 1);
    let body = document.body().as_str();
    assert!(body.contains("id=\"mermaid-1\""));
    assert!(body.contains("id=\"marknexia-text-diagram-1\""));
    // The text diagram has no `.mermaid` element for the runtime to render.
    let text_shell = &body[body.find("marknexia-text-diagram-1").unwrap()..];
    assert!(!text_shell.contains("class=\"mermaid\""));
}

// ---------------------------------------------------------------------------
// Limits and hostile inputs.
// ---------------------------------------------------------------------------

#[test]
fn oversized_blocks_fall_back_to_code() {
    let cases = [
        ("over 64 KiB", format!("{}\n", "-".repeat(500)).repeat(140)),
        ("over 400 lines", "|\n".repeat(401)),
        ("over 512 columns", "-".repeat(513)),
    ];
    for (name, code) in cases {
        let document = render(&fenced("diagram", &code));
        assert_eq!(document.text_diagrams(), 0, "{name}");
        assert!(
            document
                .body()
                .as_str()
                .contains("class=\"code-container\""),
            "{name}"
        );
    }
    // The edges are converted.
    let edges = ["|\n".repeat(399) + "|", "-".repeat(512)];
    for code in edges {
        assert_eq!(render(&fenced("diagram", &code)).text_diagrams(), 1);
    }
}

#[test]
fn the_document_limit_stops_conversion_at_64_blocks() {
    let source = fenced("bob", "--->\n|\nv").repeat(70);
    let document = render(&source);
    assert_eq!(document.text_diagrams(), 64);
    let body = document.body().as_str();
    assert!(body.contains("id=\"marknexia-text-diagram-64\""));
    assert!(!body.contains("id=\"marknexia-text-diagram-65\""));
    assert_eq!(body.matches("class=\"code-container\"").count(), 6);
}

#[test]
fn hostile_text_is_encoded_and_inert() {
    let code = "+--> <script>alert(1)</script> \"q\" 'a' &amp;
|    <svg onload=alert(1)><foreignObject>x</foreignObject></svg>
v    </code></pre><img src=x onerror=alert(1)>
<a href=\"javascript:alert(1)\">x</a> --> <style>*{}</style>";
    let document = render(&fenced("diagram", code));
    assert_eq!(document.text_diagrams(), 1);
    let body = document.body().as_str();
    assert!(body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(body.contains("&lt;svg onload=alert(1)&gt;"));
    // The SVG holds only the generated paths.
    let svg = &body[body.find("<svg").unwrap()..body.find("</svg>").unwrap()];
    assert!(!svg.contains("script"));
    assert!(svg.matches("<path ").count() <= 5);
}

#[test]
fn pathological_characters_render_within_bounds() {
    let combining = format!("a{}\n|\nv", "\u{301}".repeat(30_000));
    let mixed = "漢字 ──→ 😀 ──→ \u{202e}rtl\u{202c}\n│\n▼\n\u{feff}\u{200b}x\t|\ty".to_owned();
    let controls = "\u{1}\u{7f}\u{85} --> x\n|\nv".to_owned();
    let replacement = "\u{fffd}".repeat(1000) + "\n|\nv";
    let all_box: String = (0x2500..=0x257F)
        .filter_map(char::from_u32)
        .collect::<String>()
        .repeat(4);
    let dense = "+-+-".repeat(128) + "\n" + &"|v|^".repeat(128);
    for code in [combining, mixed, controls, replacement, all_box, dense] {
        let document = render(&fenced("diagram", &code));
        assert!(document.text_diagrams() <= 1);
    }
    // A grid of dense connectors at the size limits stays within the
    // per-block output limit or falls back to code.
    let row = "+-".repeat(256);
    let grid = format!("{row}\n{}\n", "|v".repeat(256)).repeat(120);
    let document = render(&fenced("diagram", &grid));
    assert!(document.page_bytes() < 8 * 1024 * 1024);
}

#[test]
fn deep_nesting_around_a_diagram() {
    let mut source = String::new();
    for depth in 0..40 {
        source.push_str(&"> ".repeat(depth));
        source.push_str("- item\n");
    }
    source.push_str(&"> ".repeat(40));
    source.push_str("```diagram\n");
    for line in ["a --> b", "|", "v"] {
        source.push_str(&"> ".repeat(40));
        source.push_str(line);
        source.push('\n');
    }
    source.push_str(&"> ".repeat(40));
    source.push_str("```\n");
    let document = render(&source);
    assert!(document.text_diagrams() <= 1);
}

#[test]
fn cancellation_stops_before_a_conversion() {
    let source = fenced("diagram", "a --> b\n|\nv");
    let result = Renderer::new().render_cancellable(&source, &context(), &|| true);
    assert_eq!(result.unwrap_err(), RenderError::Cancelled);
    // Polls happen only for eligible blocks: count them.
    let polls = std::sync::atomic::AtomicUsize::new(0);
    let counting = || {
        polls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        false
    };
    let many = fenced("diagram", "a --> b\n|\nv").repeat(5);
    Renderer::new()
        .render_cancellable(&many, &context(), &counting)
        .unwrap();
    assert!(polls.load(std::sync::atomic::Ordering::Relaxed) >= 5);
}
