//! Highlighting parity with .NET `ColorCodeSyntaxHighlighter.HighlightCode`
//! (ColorCode.Core/HTML 2.0.15) for every ColorCode language (gate G1).
//! `tests/oracle/highlight.v1.json` is the unmodified .NET output, written by
//! `tests/oracle/HighlightOracle` from the `src/` tree of the frozen fixture
//! revision; the same generator writes `src/highlight/languages.rs` and
//! `src/highlight/unicode.rs`.
//!
//! Every case must match byte for byte, except the cases listed in
//! [`KNOWN_DIFFERENCES`], which must still differ (a stale entry fails) and
//! whose Rust output is pinned in `oracle/highlight.known-differences.v1.json`.
//! Each one names its record in `compat/decisions/highlight-differences.md`.

use std::collections::BTreeMap;

use marknexia_rendering::testing::{highlight_budget, highlight_code, highlight_code_with_engine};
use serde_json::Value;

const ORACLE: &str = include_str!("oracle/highlight.v1.json");
const PINNED: &str = include_str!("oracle/highlight.known-differences.v1.json");
const CASE_COUNT: usize = 522;
const LANGUAGE_COUNT: usize = 25;

/// Case name and the decision that records the difference.
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[
    // .NET backtracks for longer than the oracle's 20 s match timeout; Rust
    // stops at the step budget and falls back to plain text.
    ("json/soup-2", "HL-1"),
    ("json/long-unterminated-quotes", "HL-1"),
    ("koka/soup-2", "HL-1"),
    ("koka/soup-5", "HL-1"),
    ("koka/long-unterminated-quotes", "HL-1"),
    // .NET finishes after about 3 s (402 million Rust steps); Rust stops at
    // its budget of about one second and falls back to plain text.
    ("json/soup-3", "HL-1"),
];

fn cases() -> Vec<Value> {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    assert_eq!(oracle["schemaVersion"], "marknexia-highlight-oracle-v1");
    // The generator pins the culture; the IgnoreCase tables depend on it (HL-3).
    assert_eq!(oracle["culture"], "en-IN");
    assert_eq!(
        oracle["sourceRevision"],
        "38b39f668ce52026261ac4802602c0cf7e0711a3"
    );
    let cases = oracle["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), CASE_COUNT);
    cases
}

fn plain(code: &str) -> String {
    let mut out = String::from("<pre><code>");
    for c in code.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\u{a0}'..='\u{ff}' | '\u{10000}'.. => out.push_str(&format!("&#{};", u32::from(c))),
            _ => out.push(c),
        }
    }
    out.push_str("</code></pre>");
    out
}

#[test]
fn highlighting_matches_the_dotnet_oracle() {
    let cases = cases();
    let known: BTreeMap<&str, &str> = KNOWN_DIFFERENCES.iter().copied().collect();
    let mut failures = Vec::new();
    let mut languages = std::collections::BTreeSet::new();
    let mut exact = 0;
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let language = case["language"].as_str().unwrap();
        let code = case["code"].as_str().unwrap();
        if let Some((id, _)) = name.split_once('/')
            && id != "resolve"
        {
            languages.insert(id.to_owned());
        }
        let (actual, steps) = highlight_code(code, language);
        let budget = highlight_budget(code.encode_utf16().count() as u64);
        let same = case["html"].as_str() == Some(actual.as_str());
        if same {
            exact += 1;
        }
        match (same, known.get(name)) {
            (true, None) | (false, Some(_)) => {}
            (true, Some(decision)) => failures.push(format!(
                "{name}: matches .NET now; remove the stale {decision} entry"
            )),
            (false, None) => failures.push(format!(
                "{name} ({language:?}) steps {steps}/{budget}:\n  expected: {}\n  actual:   {actual}",
                case["html"].as_str().unwrap_or("(timeout)")
            )),
        }
    }
    eprintln!(
        "highlight oracle: {exact}/{} exact, {} known differences",
        cases.len(),
        known.len()
    );
    assert_eq!(languages.len(), LANGUAGE_COUNT, "{languages:?}");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The documented differences, pinned byte for byte.
#[test]
fn known_highlight_differences_render_as_recorded() {
    let pinned: Value = serde_json::from_str(PINNED).unwrap();
    let pinned = pinned.as_object().unwrap();
    assert_eq!(pinned.len(), KNOWN_DIFFERENCES.len());
    let cases = cases();
    for (name, decision) in KNOWN_DIFFERENCES {
        let case = cases.iter().find(|case| case["name"] == *name).unwrap();
        let code = case["code"].as_str().unwrap();
        let (actual, steps) = highlight_code(code, case["language"].as_str().unwrap());
        let entry = &pinned[*name];
        assert_eq!(entry["decision"], *decision, "{name}");
        assert_eq!(actual, entry["rustHtml"].as_str().unwrap(), "{name}");
        // HL-1: .NET timed out or highlighted slowly; Rust spent its whole
        // budget and fell back to the plain .NET fallback.
        assert!(
            case["outcome"] == "timeout" || case["html"].as_str() != Some(&actual),
            "{name}"
        );
        assert_eq!(actual, plain(code), "{name}");
        let units = code.encode_utf16().count() as u64;
        assert_eq!(steps, highlight_budget(units), "{name}");
    }
}

/// C# normally runs through the linear fast path; the regex engine must
/// produce the same .NET output for every C# case.
#[test]
fn csharp_engine_path_matches_the_oracle() {
    let mut checked = 0;
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        let language = case["language"].as_str().unwrap();
        let resolves_to_csharp =
            name.starts_with("c#/") || ["C#", " cs ", "CSharp", "cake", "C#\t"].contains(&language);
        if !resolves_to_csharp {
            continue;
        }
        let (actual, _) = highlight_code_with_engine(case["code"].as_str().unwrap(), language);
        assert_eq!(case["html"].as_str(), Some(actual.as_str()), "{name}");
        checked += 1;
    }
    assert!(checked > 20, "{checked}");
}

/// Ordinary code stays far from the budget, so only pathological
/// backtracking reaches it (HL-1). Long ordinary lines need at most half the
/// per-unit credit; short ordinary cases at most a sixteenth of the fixed
/// part. Token soups and long unterminated quotes are the pathological
/// shapes and are excluded.
#[test]
fn ordinary_code_uses_a_small_part_of_the_budget() {
    let base = highlight_budget(0);
    let per_unit_credit = highlight_budget(1) - base;
    let mut worst_long = (0u64, String::new());
    let mut worst_short = (0u64, String::new());
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        if name.contains("/soup-") || name.ends_with("/long-unterminated-quotes") {
            continue;
        }
        let code = case["code"].as_str().unwrap();
        let (_, steps) = highlight_code(code, case["language"].as_str().unwrap());
        let units = code.encode_utf16().count() as u64;
        if units >= 1_000 {
            worst_long = worst_long.max((steps / units, name.to_owned()));
        } else {
            worst_short = worst_short.max((steps, name.to_owned()));
        }
    }
    eprintln!(
        "long lines: at most {worst_long:?} steps per unit; short cases: at most {worst_short:?} steps"
    );
    assert!(worst_long.0 <= per_unit_credit / 2, "{worst_long:?}");
    assert!(worst_short.0 <= base / 16, "{worst_short:?}");
}
/// Steps per case without a budget limit (run with `--ignored`).
#[test]
#[ignore = "budget report"]
fn budget_report() {
    let mut rows = Vec::new();
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        if KNOWN_DIFFERENCES.iter().any(|(known, _)| *known == name) {
            continue;
        }
        let code = case["code"].as_str().unwrap();
        let units = code.encode_utf16().count() as u64;
        let started = std::time::Instant::now();
        let (html, steps) = marknexia_rendering::testing::highlight_code_with_budget(
            code,
            case["language"].as_str().unwrap(),
            1 << 34,
        );
        let elapsed = started.elapsed();
        assert_eq!(case["html"].as_str(), Some(html.as_str()), "{name}");
        rows.push((steps, units, elapsed, name.to_owned()));
    }
    rows.sort();
    for (steps, units, elapsed, name) in rows.iter().rev().take(25) {
        eprintln!(
            "{name}: {steps} steps, {units} units, {:.0}/unit, {elapsed:?}",
            *steps as f64 / (*units).max(1) as f64
        );
    }
    let total: u64 = rows.iter().map(|r| r.0).sum();
    let time: std::time::Duration = rows.iter().map(|r| r.2).sum();
    eprintln!("total {total} steps in {time:?}");
}
