//! Frozen .NET sanitizer fixtures as hard gates.
//!
//! Every `compat/fixtures/v1/sanitizer/*.case.json` case must either match
//! the .NET output byte-for-byte or be listed in [`DECISIONS`] against a named
//! entry in the proposed compatibility decision file. A listed case that
//! starts matching exactly fails too, so stale decisions cannot linger.

use std::{fs, path::PathBuf};

use marknexia_security::{ContentPolicy, HtmlPolicy, SvgPolicy};
use serde::Deserialize;

const DECISION_FILE: &str = "compat/decisions/sanitizer-ammonia-differences.md";

/// (case name, decision id, exact Rust output).
const DECISIONS: &[(&str, &str, &str)] = &[("unsafe-css", "SAN-1", "<div>text</div>")];

#[derive(Deserialize)]
struct Fixture {
    area: String,
    name: String,
    input: FixtureInput,
    expected: FixtureExpected,
}

#[derive(Deserialize)]
struct FixtureInput {
    html: String,
    mode: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureExpected {
    sanitized_html: String,
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sanitize(mode: &str, input: &str) -> String {
    let policy = ContentPolicy::default();
    match mode {
        "html" => HtmlPolicy::new(policy)
            .sanitize_fragment(input)
            .expect("fixture input is within limits")
            .into_string(),
        "svg" => SvgPolicy::new(policy)
            .sanitize(input)
            .expect("fixture input is within limits")
            .as_str()
            .to_owned(),
        other => panic!("unexpected fixture mode {other}"),
    }
}

#[test]
fn every_frozen_sanitizer_fixture_matches_or_names_a_decision() {
    let directory = repository_root().join("compat/fixtures/v1/sanitizer");
    let decision_text = fs::read_to_string(repository_root().join(DECISION_FILE))
        .expect("decision file must exist");
    let mut seen = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&directory)
        .expect("frozen sanitizer fixtures")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".case.json"))
        })
        .collect();
    entries.sort();
    for path in entries {
        let source = fs::read_to_string(&path).expect("fixture readable");
        let case: Fixture = serde_json::from_str(&source).expect("frozen fixture JSON");
        assert_eq!(case.area, "sanitizer", "{}", path.display());
        let actual = sanitize(&case.input.mode, &case.input.html);
        match DECISIONS.iter().find(|(name, _, _)| *name == case.name) {
            None => assert_eq!(
                actual, case.expected.sanitized_html,
                "{} must match the frozen .NET output exactly",
                case.name
            ),
            Some((_, decision, rust_expected)) => {
                assert!(
                    decision_text.contains(&format!("## {decision} ")),
                    "{decision} missing from {DECISION_FILE}"
                );
                assert!(
                    decision_text.contains(&format!("`{}`", case.name)),
                    "{decision} must name fixture {}",
                    case.name
                );
                assert!(
                    decision_text.contains(&case.expected.sanitized_html),
                    "{decision} must quote the frozen .NET output"
                );
                assert_ne!(
                    actual, case.expected.sanitized_html,
                    "{} now matches exactly; remove its decision entry",
                    case.name
                );
                assert_eq!(
                    actual, *rust_expected,
                    "{} Rust output drifted from {decision}",
                    case.name
                );
            }
        }
        seen.push(case.name);
    }
    for expected in ["unsafe-css", "unsafe-html", "unsafe-svg", "unsafe-uri"] {
        assert!(
            seen.iter().any(|name| name == expected),
            "fixture {expected} missing"
        );
    }
    for (name, _, _) in DECISIONS {
        assert!(
            seen.iter().any(|seen| seen == name),
            "decision references unknown fixture {name}"
        );
    }
}

/// Sanitizer-visible fragments taken from the frozen rendering fixtures
/// (`compat/fixtures/v1/rendering`). The .NET output ran through Ganss, so the
/// Rust sanitizer must leave them byte-identical.
#[test]
fn rendered_markdown_fragments_from_rendering_fixtures_are_preserved() {
    let fragments = [
        "<div class=\"code-container\">\n  <button type=\"button\" class=\"copy-btn\" data-marknexia-action=\"copy\" data-copy-text=\"var%20value%20%3D%201%3B%0A\" title=\"Copy code\">Copy</button>\n  <div class=\"csharp\"><pre><span class=\"keyword\">var</span> value = <span class=\"number\">1</span>;\n\n</pre></div>\n</div>",
        "<div class=\"marknexia-diagram-zoom-status\" data-marknexia-zoom-status=\"\" aria-live=\"polite\">100%</div>",
        "<div class=\"marknexia-diagram-source\" id=\"mermaid-1-source\" style=\"display: none\">\n    <pre><code>graph TD\nA--&gt;B\n</code></pre>\n  </div>",
        "<button type=\"button\" class=\"marknexia-btn\" data-marknexia-action=\"zoom-out\" title=\"Zoom out\" aria-label=\"Zoom out\">\u{2212}</button>",
        "<div class=\"marknexia-diagram-fallback\" role=\"note\"><strong>Mermaid diagram was not rendered.</strong>\n  <p>diagram limit exceeded.</p>\n  <details><summary>Show diagram source</summary><pre><code>graph TD\nB--&gt;C\n</code></pre></details>\n</div>",
        "<div class=\"marknexia-diagram-canvas\" id=\"mermaid-1-canvas\"><pre class=\"mermaid\" id=\"mermaid-1-render\">graph TD\nA--&gt;B\n</pre></div>",
        "<p><img src=\"\" alt=\"remote\"></p>",
    ];
    let html = HtmlPolicy::new(ContentPolicy::default());
    for fragment in fragments {
        assert_eq!(html.sanitize_fragment(fragment).unwrap().as_str(), fragment);
    }
}
