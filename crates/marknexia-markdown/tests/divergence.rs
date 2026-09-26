//! Informational probe of syntax the frozen corpus does not exercise. There is
//! no .NET oracle for these inputs, so this is not a parity gate; it lists
//! where the two candidates disagree through the shared Marknexia layer.
//! Run with:
//! `cargo test -p marknexia-markdown --all-features -- --ignored candidate_divergence_probe --nocapture`

#![cfg(all(feature = "candidate-comrak", feature = "candidate-pulldown"))]

use marknexia_markdown::{ComrakAdapter, MarkdownEngine, MarkdownOptions, PulldownAdapter};

const PROBES: &[(&str, &str)] = &[
    (
        "bare-url-autolink",
        "Visit https://example.com and www.example.com.\n",
    ),
    ("inserted", "++inserted++\n"),
    ("subscript-superscript", "H~2~O and x^2^\n"),
    ("single-tilde", "~one~ ~~two~~\n"),
    ("block-math", "$$\nx = 1\n$$\n"),
    ("entity-in-heading", "# A &amp; B &copy;\n"),
    (
        "html-details",
        "<details>\n<summary>S</summary>\n\nBody\n</details>\n",
    ),
    ("nested-lists", "- a\n  - b\n    1. c\n- d\n"),
    (
        "reference-links",
        "[x][ref] ![y][img]\n\n[ref]: /r \"T\"\n[img]: /i.png\n",
    ),
    ("pipe-table-edges", "a | b\n--|--\n`c|d` | e \\| f\n| g |\n"),
    (
        "footnote-multi-paragraph",
        "x[^n]\n\n[^n]: one\n\n    two\n",
    ),
    ("unclosed-fence", "```js\nlet a = 1;\n"),
    ("hard-breaks", "a\\\nb  \nc\n"),
    ("emphasis-edges", "*foo**bar* __a__b ***c***\n"),
    ("ordered-zero", "0. zero\n1. one\n"),
    ("tabs", "-\tone\n\n\tcode\n"),
    (
        "html-inline-anchor-attrs",
        "<a class=\"x\" id=\"first\" name=\"second\">t</a>\n",
    ),
    ("mark-edges", "== spaced == a==b== ==*x*==\n"),
    ("task-variants", "- [X] upper\n- [ ]\n* [x] star\n"),
    ("setext-multiline", "Line one\nline two\n---\n"),
    ("link-in-heading", "## [Docs](d.md) `code` *em*\n"),
    // Review I2: escaped `=` must never become a mark delimiter.
    ("escaped-mark", "\\==a\\== and \\=\\=b\\=\\=\n"),
    ("plus-mark-plus", "+==+==+\n"),
    ("escaped-backslash-mark", "\\\\==a== x\\\\\\==b==\n"),
    // Review I3: non-ASCII grid tables (Markdig measures UTF-16 code units).
    (
        "grid-non-ascii",
        "+--+--+\n|é |😀|\n+==+==+\n|a |b |\n+--+--+\n",
    ),
    ("grid-astral-width", "+----+\n|😀|\n+----+\n"),
];

fn render(engine: &dyn MarkdownEngine, source: &str) -> String {
    let parsed = engine.parse(source, &MarkdownOptions::default()).unwrap();
    format!(
        "{}links={:?} images={:?} headings={:?} anchors={:?}",
        parsed.rendered_body_html,
        parsed.links,
        parsed.images,
        parsed
            .headings
            .iter()
            .map(|heading| (&heading.text, &heading.slug_id))
            .collect::<Vec<_>>(),
        parsed
            .custom_anchors
            .iter()
            .map(|anchor| &anchor.id)
            .collect::<Vec<_>>(),
    )
}

#[test]
#[ignore = "informational; no oracle"]
fn candidate_divergence_probe() {
    let mut divergent = 0;
    for (name, source) in PROBES {
        let comrak = render(&ComrakAdapter, source);
        let pulldown = render(&PulldownAdapter, source);
        if comrak != pulldown {
            divergent += 1;
            println!("--- {name}\n  comrak:   {comrak:?}\n  pulldown: {pulldown:?}");
        }
    }
    println!("{divergent} of {} probes diverge", PROBES.len());
}
