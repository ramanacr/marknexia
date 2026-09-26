//! Frozen .NET sanitizer fixtures as hard gates.
//!
//! Every `compat/fixtures/v1/sanitizer/*.case.json` case must either match
//! the .NET output byte-for-byte or be listed in [`DECISIONS`] against a named
//! entry in the proposed compatibility decision file. A listed case that
//! starts matching exactly fails too, so stale decisions cannot linger.

use std::{
    fs,
    path::{Path, PathBuf},
};

use marknexia_security::{ContentPolicy, HtmlPolicy, PolicyLimits, RemoteImagePolicy, SvgPolicy};
use serde::Deserialize;

const DECISION_FILE: &str = "compat/decisions/sanitizer-ammonia-differences.md";

/// (case name, decision id, exact Rust output).
const DECISIONS: &[(&str, &str, &str)] = &[("unsafe-css", "SAN-1", "<div>text</div>")];

const SCHEMA_VERSION: &str = "marknexia-parity-v1";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    area: String,
    schema_version: String,
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

/// Every entry in `directory` must be a `*.case.json` file; anything else fails.
fn case_files(directory: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .expect("frozen fixture directory")
        .map(|entry| entry.expect("directory entry").path())
        .collect();
    for path in &entries {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        assert!(
            path.is_file() && name.ends_with(".case.json"),
            "unexpected entry in {}: {}",
            directory.display(),
            path.display()
        );
    }
    entries.sort();
    entries
}

/// The section of the decision file from `## {decision} ` up to the next
/// newline-`##` heading.
fn decision_section<'a>(text: &'a str, decision: &str) -> &'a str {
    let heading = format!("## {decision} ");
    let start = text
        .find(&heading)
        .unwrap_or_else(|| panic!("{decision} missing from {DECISION_FILE}"));
    let rest = &text[start..];
    let end = rest[heading.len()..]
        .find("\n## ")
        .map_or(rest.len(), |offset| heading.len() + offset);
    &rest[..end]
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
    for path in case_files(&directory) {
        let source = fs::read_to_string(&path).expect("fixture readable");
        let case: Fixture = serde_json::from_str(&source).expect("frozen fixture JSON");
        assert_eq!(case.area, "sanitizer", "{}", path.display());
        assert_eq!(case.schema_version, SCHEMA_VERSION, "{}", path.display());
        let actual = sanitize(&case.input.mode, &case.input.html);
        match DECISIONS.iter().find(|(name, _, _)| *name == case.name) {
            None => assert_eq!(
                actual, case.expected.sanitized_html,
                "{} must match the frozen .NET output exactly",
                case.name
            ),
            Some((_, decision, rust_expected)) => {
                let section = decision_section(&decision_text, decision);
                assert!(
                    section.contains(&format!("`{}`", case.name)),
                    "{decision} must name fixture {}",
                    case.name
                );
                assert!(
                    section.contains(&case.expected.sanitized_html),
                    "{decision} must quote the frozen .NET output"
                );
                assert!(
                    section.contains(&format!("`{rust_expected}`")),
                    "{decision} must quote the Rust output"
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenderingFixture {
    schema_version: String,
    name: String,
    input: RenderingInput,
    expected: RenderingExpected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenderingInput {
    #[serde(default)]
    allow_remote_assets: bool,
}

#[derive(Deserialize)]
struct RenderingExpected {
    outcome: String,
    #[serde(default)]
    html: Option<String>,
}

const BODY_OPEN: &str = "<div class=\"markdown-body\">";

/// The rendered Markdown body of a frozen .NET page: the content of
/// `<div class="markdown-body">`, which is what went through Ganss.
fn markdown_body(page: &str) -> &str {
    let start = page.find(BODY_OPEN).expect("markdown-body container") + BODY_OPEN.len();
    let scripts = page[start..]
        .find("<script")
        .map_or(page.len(), |offset| start + offset);
    let end = page[start..scripts]
        .rfind("</div>")
        .map(|offset| start + offset)
        .expect("markdown-body close");
    &page[start..end]
}

/// Every rendered page in `compat/fixtures/v1/rendering` carries the .NET
/// sanitizer's output as its Markdown body. Sanitizing that body again with
/// the Rust policy (with the fixture's remote-image setting) must leave it
/// byte-identical, so fixture drift or policy drift fails here.
#[test]
fn rendered_markdown_bodies_from_rendering_fixtures_are_preserved() {
    let directory = repository_root().join("compat/fixtures/v1/rendering");
    let mut rendered = 0;
    for path in case_files(&directory) {
        let source = fs::read_to_string(&path).expect("fixture readable");
        let case: RenderingFixture = serde_json::from_str(&source).expect("frozen fixture JSON");
        assert_eq!(case.schema_version, SCHEMA_VERSION, "{}", path.display());
        if case.expected.outcome != "rendered" {
            continue;
        }
        let page = case.expected.html.as_deref().expect("rendered page html");
        let body = markdown_body(page);
        let remote = if case.input.allow_remote_assets {
            RemoteImagePolicy::AllowHttps
        } else {
            RemoteImagePolicy::Deny
        };
        let policy = ContentPolicy::try_new(PolicyLimits::default(), remote).unwrap();
        let sanitized = HtmlPolicy::new(policy)
            .sanitize_fragment(body)
            .expect("rendered body within limits");
        assert_eq!(sanitized.as_str(), body, "{} body changed", case.name);
        rendered += 1;
    }
    assert!(
        rendered >= 5,
        "expected the rendered fixtures, found {rendered}"
    );
}
