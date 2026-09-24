#![cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]

use marknexia_markdown::{MarkdownEngine, MarkdownOptions};
use serde_json::Value;
use std::{fs, path::PathBuf};

fn fixture(name: &str) -> (String, Value) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compat/fixtures/v1")
        .join(name);
    let case: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    (
        case["input"]["markdown"].as_str().unwrap().to_owned(),
        case["expected"].clone(),
    )
}

fn check_heading_ids(engine: &dyn MarkdownEngine) {
    let (source, expected) = fixture("headings/duplicate-formatted-headings.case.json");
    let actual = serde_json::to_value(engine.parse(&source, &MarkdownOptions).unwrap()).unwrap();
    assert_eq!(actual["headings"], expected["headings"]);
    assert_eq!(actual["renderedBodyHtml"], expected["renderedBodyHtml"]);
}

fn check_custom_anchor(engine: &dyn MarkdownEngine) {
    for case in [
        "headings/custom-anchor.case.json",
        "markdown/navigation-docs-encoded-name.case.json",
    ] {
        let (source, expected) = fixture(case);
        let actual =
            serde_json::to_value(engine.parse(&source, &MarkdownOptions).unwrap()).unwrap();
        assert_eq!(actual["customAnchors"], expected["customAnchors"], "{case}");
    }
}

fn check_mermaid(engine: &dyn MarkdownEngine) {
    for case in [
        "headings/mermaid-block.case.json",
        "markdown/diagrams-mermaid.case.json",
    ] {
        let (source, expected) = fixture(case);
        let actual =
            serde_json::to_value(engine.parse(&source, &MarkdownOptions).unwrap()).unwrap();
        assert_eq!(actual["diagrams"], expected["diagrams"], "{case}");
    }
}

#[cfg(feature = "candidate-comrak")]
#[test]
fn comrak_extensions_follow_frozen_fields() {
    let engine = marknexia_markdown::ComrakAdapter;
    check_heading_ids(&engine);
    check_custom_anchor(&engine);
    check_mermaid(&engine);
}

#[cfg(feature = "candidate-pulldown")]
#[test]
fn pulldown_extensions_follow_frozen_fields() {
    let engine = marknexia_markdown::PulldownAdapter;
    check_heading_ids(&engine);
    check_custom_anchor(&engine);
    check_mermaid(&engine);
}
