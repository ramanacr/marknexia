//! Marknexia-owned extension behavior beyond the frozen corpus, checked
//! identically for every candidate through the public engine API.

#![cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]

use marknexia_core::contracts::DiagnosticSeverity;
use marknexia_markdown::{MarkdownEngine, MarkdownOptions, ParsedDocument};

fn parse(engine: &dyn MarkdownEngine, source: &str) -> ParsedDocument {
    engine.parse(source, &MarkdownOptions::default()).unwrap()
}

fn check_mermaid_isolation_and_limits(engine: &dyn MarkdownEngine) {
    let source =
        "```Mermaid extra\r\ngraph TD\r\n\r\nA-->B\r\n```\r\n\r\n```mermaid\nsecond\n```\n";
    let parsed = parse(engine, source);
    assert_eq!(parsed.diagrams.len(), 2);
    assert_eq!(parsed.diagrams[0].id, "mermaid-1");
    assert_eq!(parsed.diagrams[0].source_code, "graph TD\r\nA-->B");
    assert_eq!(parsed.diagrams[0].line_number, 0);
    assert_eq!(parsed.diagrams[1].line_number, 6);
    assert!(parsed.rendered_body_html.starts_with(
        "<pre><code class=\"language-Mermaid\">graph TD\n\nA--&gt;B\n</code></pre>\n"
    ));
    assert!(parsed.diagnostics.is_empty());

    let limited = engine
        .parse(
            source,
            &MarkdownOptions {
                max_diagram_count: 1,
                max_diagram_source_bytes: 8,
            },
        )
        .unwrap();
    let messages: Vec<_> = limited
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.message.as_str(), diagnostic.source_line))
        .collect();
    assert_eq!(
        messages,
        [
            ("mermaid-1: diagram source limit exceeded", Some(0)),
            ("mermaid-2: diagram limit exceeded", Some(6)),
        ]
    );
    assert!(
        limited
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
    );
    assert_eq!(
        limited.diagrams, parsed.diagrams,
        "limits never drop extraction"
    );
}

fn check_link_classification(engine: &dyn MarkdownEngine) {
    let parsed = parse(
        engine,
        "# See <https://a.example> and [b](https://b.example)\n\n![alt *x*](i.png \"t\") <me@example.com>\n",
    );
    assert_eq!(parsed.links, ["https://b.example"]);
    assert_eq!(parsed.images, ["i.png"]);
    assert_eq!(parsed.headings[0].text, "See  and b");
    assert!(
        parsed
            .rendered_body_html
            .contains("<a href=\"https://a.example\">https://a.example</a>")
    );
    assert!(
        parsed
            .rendered_body_html
            .contains("<img src=\"i.png\" alt=\"alt x\" title=\"t\" />")
    );
    assert!(
        parsed
            .rendered_body_html
            .contains("<a href=\"mailto:me@example.com\">me@example.com</a>")
    );
}

fn check_structure_markup(engine: &dyn MarkdownEngine) {
    let parsed = parse(
        engine,
        "3. first\n4. second\n\n- a\n\n  b\n- [ ] c\n\n> ==mark *x*==\n\n+---+----+---+\n| a | bb | c |\n+---+----+---+\n",
    );
    assert_eq!(
        parsed.rendered_body_html,
        "<ol start=\"3\">\n<li>first</li>\n<li>second</li>\n</ol>\n\
<ul class=\"contains-task-list\">\n<li><p>a</p>\n<p>b</p>\n</li>\n\
<li class=\"task-list-item\"><p><input disabled=\"disabled\" type=\"checkbox\" /> c</p>\n</li>\n</ul>\n\
<blockquote>\n<p><mark>mark <em>x</em></mark></p>\n</blockquote>\n\
<table>\n<col style=\"width:30%\" />\n<col style=\"width:40%\" />\n<col style=\"width:30%\" />\n\
<tbody>\n<tr>\n<td>a</td>\n<td>bb</td>\n<td>c</td>\n</tr>\n</tbody>\n</table>\n"
    );
}

fn check_heading_ids(engine: &dyn MarkdownEngine) {
    let parsed = parse(engine, "# Café `Code`\n\n## Café Code\n\nSetext\n======\n");
    let ids: Vec<_> = parsed
        .headings
        .iter()
        .map(|heading| heading.slug_id.as_str())
        .collect();
    assert_eq!(ids, ["café-code", "café-code-1", "setext"]);
    assert!(parsed.rendered_body_html.starts_with(
        "<h1 id=\"café-code\">Café <code>Code</code></h1>\n<h2 id=\"café-code-1\">Café Code</h2>\n<h1 id=\"setext\">Setext</h1>\n"
    ));
    assert_eq!(parsed.headings[2].line_number, 4);
}

fn check_all(engine: &dyn MarkdownEngine) {
    assert_eq!(parse(engine, ""), ParsedDocument::default());
    check_mermaid_isolation_and_limits(engine);
    check_link_classification(engine);
    check_structure_markup(engine);
    check_heading_ids(engine);
}

#[cfg(feature = "candidate-comrak")]
#[test]
fn comrak_follows_marknexia_extension_contract() {
    check_all(&marknexia_markdown::ComrakAdapter);
}

#[cfg(feature = "candidate-pulldown")]
#[test]
fn pulldown_follows_marknexia_extension_contract() {
    check_all(&marknexia_markdown::PulldownAdapter);
}
