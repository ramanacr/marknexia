//! Math rendering parity with the .NET `SimpleMathRenderer`, through the
//! whole pipeline (Markdown math spans, the math pass, the sanitizer and the
//! template). `tests/oracle/math.v1.json` is the unmodified .NET
//! `MarkdownRenderer` output, written by `tests/oracle/MathOracle` from the
//! `src/` tree of the frozen fixture revision.
//!
//! Every case must match byte for byte, except the cases listed in
//! [`KNOWN_DIFFERENCES`], which must still differ (a stale entry fails).
//! Each one names its record: REND-5 in `compat/decisions/rendering-differences.md`,
//! MATH-1 to MATH-3 in `compat/decisions/math-differences.md`.

use std::collections::BTreeMap;

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, Renderer};
use serde_json::Value;

const ORACLE: &str = include_str!("oracle/math.v1.json");
const CASE_COUNT: usize = 43;
const DOCUMENT_ID: [u8; 16] = [0x5a; 16];
const NONCE: [u8; 16] = [
    0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0x01,
];

/// Case name and the decision that records the difference.
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[
    // .NET recurses without a bound; Rust emits content past the element
    // budget as encoded text.
    ("deep-nesting", "REND-5 (MAX_MATH_DEPTH)"),
    // .NET throws ArgumentOutOfRangeException and the render fails; Rust
    // clamps the argument to `\`.
    ("caret-backslash-at-end", "REND-5 (`^\\` at the end)"),
    ("underscore-backslash-at-end", "REND-5 (`_\\` at the end)"),
    // .NET walks UTF-16 code units, so an astral letter splits into two
    // lone surrogates that become U+FFFD; Rust keeps the character.
    ("astral", "MATH-3"),
    // Markdig math blocks (`$$` fences) become `<div class="math">\n\[…\]`;
    // pulldown-cmark reads them as inline display math in a paragraph.
    ("display-block", "MATH-1"),
    ("multi-line-display", "MATH-1"),
    ("math-disabled", "MATH-1"),
    // Markdig and pulldown-cmark recognize different inline `$` spans.
    ("unbalanced", "MATH-2"),
    ("leading-space", "MATH-2"),
];

fn identity() -> PageIdentity {
    PageIdentity::new(DOCUMENT_ID, NONCE).unwrap()
}

fn normalize(page: &str) -> String {
    let identity = identity();
    let host = identity.origin().trim_start_matches("https://").to_owned();
    page.replace(&host, "document-{volatile}.marknexia.viewer")
        .replace(&identity.nonce(), "{volatile}")
}

fn body(page: &str) -> &str {
    const OPEN: &str = "<div class=\"markdown-body\">\n";
    const CLOSE: &str = "\n  </div>\n  <script";
    let start = page.find(OPEN).unwrap() + OPEN.len();
    let end = page.rfind(CLOSE).unwrap();
    &page[start..end]
}

/// `rendered`, `body`, optional full page; or `error` and its debug text.
fn render(case: &Value) -> (String, Option<String>, Option<String>) {
    let mut context = RenderContext::new(AppTheme::System, identity());
    context.enable_math = case["enableMath"].as_bool().unwrap();
    context.document_directory = "compat".to_owned();
    match Renderer::new().render(case["markdown"].as_str().unwrap(), &context) {
        Ok(document) => {
            let page = normalize(&document.page_html());
            (
                "rendered".to_owned(),
                Some(body(&page).to_owned()),
                Some(page),
            )
        }
        Err(error) => (format!("{error:?}"), None, None),
    }
}

fn oracle() -> Vec<Value> {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    assert_eq!(oracle["schemaVersion"], "marknexia-math-oracle-v1");
    // The fixture revision of compat/fixtures/v1; its `src/` tree is the
    // one recorded here.
    assert_eq!(
        oracle["sourceRevision"],
        "38b39f668ce52026261ac4802602c0cf7e0711a3"
    );
    oracle["cases"].as_array().unwrap().clone()
}

#[test]
fn math_matches_the_dotnet_oracle() {
    let cases = oracle();
    assert_eq!(cases.len(), CASE_COUNT);
    let known: BTreeMap<&str, &str> = KNOWN_DIFFERENCES.iter().copied().collect();
    let mut failures = Vec::new();
    let mut pages = 0;
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let (outcome, actual_body, actual_page) = render(case);
        let same = match case["outcome"].as_str().unwrap() {
            "rendered" => {
                let expected = case["body"].as_str().unwrap();
                let page_matches = case.get("page").is_none_or(|page| {
                    pages += 1;
                    actual_page.as_deref() == page.as_str()
                });
                outcome == "rendered" && actual_body.as_deref() == Some(expected) && page_matches
            }
            // .NET failed the whole render.
            _ => outcome != "rendered",
        };
        match (same, known.get(name)) {
            (true, None) | (false, Some(_)) => {}
            (true, Some(decision)) => failures.push(format!(
                "{name}: matches .NET now; remove the stale {decision} entry"
            )),
            (false, None) => failures.push(format!(
                "{name}:\n  expected ({}): {}\n  actual   ({outcome}): {}",
                case["outcome"],
                case["body"]
                    .as_str()
                    .or(case["exception"].as_str())
                    .unwrap_or(""),
                actual_body.as_deref().unwrap_or("")
            )),
        }
    }
    assert_eq!(pages, 1, "one case carries the complete page");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    for name in known.keys() {
        assert!(
            cases.iter().any(|case| case["name"] == *name),
            "{name} is not an oracle case"
        );
    }
}

/// The documented differences, pinned byte for byte to the Rust body in
/// `oracle/math.known-differences.v1.json`, so any change to them fails.
/// The checks after the loop explain what the pinned bodies show.
#[test]
fn known_math_differences_render_as_recorded() {
    let pinned: Value =
        serde_json::from_str(include_str!("oracle/math.known-differences.v1.json")).unwrap();
    let pinned = pinned.as_object().unwrap();
    assert_eq!(pinned.len(), KNOWN_DIFFERENCES.len());
    let cases = oracle();
    let body = |name: &str| {
        let case = cases.iter().find(|case| case["name"] == name).unwrap();
        render(case).1.unwrap()
    };
    for (name, decision) in KNOWN_DIFFERENCES {
        let entry = &pinned[*name];
        assert_eq!(entry["decision"], *decision, "{name}");
        assert_eq!(
            body(name),
            entry["rustBody"].as_str().unwrap(),
            "{name}: the Rust output for a recorded difference changed"
        );
    }
    assert_eq!(
        body("caret-backslash-at-end"),
        "<span class=\"marknexia-math\" role=\"math\" aria-label=\"x^\\\">x<sup>\\</sup></span>\n"
    );
    assert_eq!(
        body("underscore-backslash-at-end"),
        "<div class=\"marknexia-math-display\" role=\"math\" aria-label=\"y_\\\">y<sub>\\</sub></div>\n"
    );
    assert!(body("astral").contains("\u{1d465}<sup>2</sup> + \\\u{1d465}"));
    let deep = body("deep-nesting");
    assert_eq!(deep.matches("class=\"marknexia-sqrt\"").count(), 17);
    assert!(
        deep.contains("\\sqrt{"),
        "content past the budget stays text"
    );
}
