//! Mandatory Markdown parity gate. Every candidate runs the same frozen .NET
//! oracle cases (`compat/fixtures/v1/{markdown,headings}`); there are no
//! candidate-specific expected files. A difference fails the gate unless a
//! `compat/decisions/*.md` file with status "proposed — requires Task 8
//! approval" records that exact candidate, case, field, expected value, and
//! actual value. Stale allowances fail too, so decisions cannot drift.

#[cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]
use marknexia_markdown::{MarkdownEngine, MarkdownOptions};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::PathBuf};

const PROPOSED_STATUS: &str = "proposed — requires Task 8 approval";
const ALLOWANCE_OPEN: &str = "<!-- parity-allowance";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_root() -> PathBuf {
    repository_root().join("compat/fixtures/v1")
}

fn manifest_case_paths() -> Vec<String> {
    let manifest: Value =
        serde_json::from_slice(&fs::read(fixture_root().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["schemaVersion"], "marknexia-parity-v1");
    let all_cases = manifest["cases"].as_array().unwrap();
    assert_eq!(
        manifest["caseCount"].as_u64().unwrap() as usize,
        all_cases.len()
    );
    let mut paths: Vec<String> = all_cases
        .iter()
        .map(|case| case.as_str().unwrap())
        .filter(|case| case.starts_with("markdown/") || case.starts_with("headings/"))
        .map(str::to_owned)
        .collect();
    paths.sort();
    assert!(paths.iter().all(|path| path.ends_with(".case.json")));
    assert!(
        paths.windows(2).all(|pair| pair[0] != pair[1]),
        "duplicate manifest case"
    );
    paths
}

fn directory_case_paths() -> Vec<String> {
    let mut paths = Vec::new();
    for area in ["markdown", "headings"] {
        for entry in fs::read_dir(fixture_root().join(area)).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            if name.ends_with(".case.json") {
                paths.push(format!("{area}/{name}"));
            }
        }
    }
    paths.sort();
    paths
}

fn assert_membership(expected: &[String], actual: &[String]) -> Result<(), String> {
    let expected_set: BTreeSet<_> = expected.iter().collect();
    let actual_set: BTreeSet<_> = actual.iter().collect();
    let mut errors = Vec::new();
    for missing in expected_set.difference(&actual_set) {
        errors.push(format!("missing: {missing}"));
    }
    for unexpected in actual_set.difference(&expected_set) {
        errors.push(format!("unexpected: {unexpected}"));
    }
    if expected_set.len() != expected.len() || actual_set.len() != actual.len() {
        errors.push("duplicate case path".to_owned());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// One observable difference between the frozen oracle and a candidate.
#[derive(Clone, Debug, PartialEq)]
struct Mismatch {
    case: String,
    path: String,
    expected: Option<Value>,
    actual: Option<Value>,
}

fn differences(case: &str, path: &str, expected: &Value, actual: &Value, out: &mut Vec<Mismatch>) {
    let mismatch = |path: String, expected: Option<&Value>, actual: Option<&Value>| Mismatch {
        case: case.to_owned(),
        path,
        expected: expected.cloned(),
        actual: actual.cloned(),
    };
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (key, value) in e {
                let child = format!("{path}.{key}");
                match a.get(key) {
                    Some(found) => differences(case, &child, value, found, out),
                    None => out.push(mismatch(child, Some(value), None)),
                }
            }
            for (key, value) in a.iter().filter(|(key, _)| !e.contains_key(*key)) {
                out.push(mismatch(format!("{path}.{key}"), None, Some(value)));
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            for index in 0..e.len().max(a.len()) {
                let child = format!("{path}[{index}]");
                match (e.get(index), a.get(index)) {
                    (Some(left), Some(right)) => differences(case, &child, left, right, out),
                    (left, right) => out.push(mismatch(child, left, right)),
                }
            }
        }
        _ if expected != actual => {
            out.push(mismatch(path.to_owned(), Some(expected), Some(actual)))
        }
        _ => {}
    }
}

/// A reviewed-but-unapproved difference recorded in a decision file.
#[derive(Clone, Debug, PartialEq)]
struct Allowance {
    decision: String,
    candidate: String,
    case: String,
    path: String,
    expected: Option<Value>,
    actual: Option<Value>,
}

fn parse_allowances(decision: &str, text: &str) -> Result<Vec<Allowance>, String> {
    let mut allowances = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(ALLOWANCE_OPEN) {
        let body_start = start + ALLOWANCE_OPEN.len();
        let end = rest[body_start..]
            .find("-->")
            .ok_or_else(|| format!("{decision}: unterminated parity-allowance"))?;
        let body = &rest[body_start..body_start + end];
        rest = &rest[body_start + end + 3..];
        let field = |name: &str| -> Result<String, String> {
            body.lines()
                .find_map(|line| line.trim().strip_prefix(&format!("{name}:")))
                .map(|value| value.trim().to_owned())
                .ok_or_else(|| format!("{decision}: allowance missing `{name}`"))
        };
        let value = |name: &str| -> Result<Option<Value>, String> {
            let raw = field(name)?;
            if raw == "absent" {
                return Ok(None);
            }
            serde_json::from_str(&raw)
                .map(Some)
                .map_err(|error| format!("{decision}: `{name}` is not JSON: {error}"))
        };
        allowances.push(Allowance {
            decision: decision.to_owned(),
            candidate: field("candidate")?,
            case: field("case")?,
            path: field("path")?,
            expected: value("expected")?,
            actual: value("actual")?,
        });
    }
    if !allowances.is_empty() && !text.contains(PROPOSED_STATUS) {
        return Err(format!(
            "{decision}: parity allowances require status `{PROPOSED_STATUS}`"
        ));
    }
    Ok(allowances)
}

fn decision_allowances() -> Vec<Allowance> {
    let directory = repository_root().join("compat/decisions");
    let mut entries: Vec<_> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .flat_map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            parse_allowances(&name, &fs::read_to_string(&path).unwrap()).unwrap()
        })
        .collect()
}

/// Fails on any mismatch without an exact allowance and on any stale allowance.
fn gate(
    candidate: &str,
    mismatches: &[Mismatch],
    allowances: &[Allowance],
) -> Result<usize, String> {
    let own: Vec<&Allowance> = allowances
        .iter()
        .filter(|allowance| allowance.candidate == candidate)
        .collect();
    let covers = |allowance: &Allowance, mismatch: &Mismatch| {
        allowance.case == mismatch.case
            && allowance.path == mismatch.path
            && allowance.expected == mismatch.expected
            && allowance.actual == mismatch.actual
    };
    let mut errors = Vec::new();
    let mut allowed = 0;
    for mismatch in mismatches {
        if own.iter().any(|allowance| covers(allowance, mismatch)) {
            allowed += 1;
        } else {
            errors.push(format!(
                "{candidate} {} {}: expected={} actual={} (no matching proposed decision)",
                mismatch.case,
                mismatch.path,
                preview(mismatch.expected.as_ref()),
                preview(mismatch.actual.as_ref())
            ));
        }
    }
    for allowance in own {
        if !mismatches
            .iter()
            .any(|mismatch| covers(allowance, mismatch))
        {
            errors.push(format!(
                "{candidate} {} {}: stale allowance in {}",
                allowance.case, allowance.path, allowance.decision
            ));
        }
    }
    if errors.is_empty() {
        Ok(allowed)
    } else {
        Err(errors.join("\n"))
    }
}

fn preview(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "absent".to_owned();
    };
    let json = serde_json::to_string(value).unwrap();
    if json.chars().count() <= 160 {
        json
    } else {
        format!(
            "{}… ({} chars)",
            json.chars().take(160).collect::<String>(),
            json.chars().count()
        )
    }
}

#[cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]
fn run_mandatory_suite(candidate: &str, engine: &dyn MarkdownEngine) {
    let cases = manifest_case_paths();
    assert_membership(&cases, &directory_case_paths()).unwrap();
    assert_eq!(cases.len(), 19);
    let mut mismatches = Vec::new();
    for case in &cases {
        let area = case.split_once('/').unwrap().0;
        let fixture: Value =
            serde_json::from_slice(&fs::read(fixture_root().join(case)).unwrap()).unwrap();
        assert_eq!(fixture["schemaVersion"], "marknexia-parity-v1");
        assert_eq!(fixture["area"], area);
        let source = fixture["input"]["markdown"].as_str().unwrap();
        let parsed = engine
            .parse(source, &MarkdownOptions::default())
            .unwrap_or_else(|diagnostics| panic!("{candidate} {case}: {diagnostics:?}"));
        // The .NET adapter never reports parser diagnostics for these cases.
        assert!(
            parsed.diagnostics.is_empty(),
            "{candidate} {case}: unexpected diagnostics {:?}",
            parsed.diagnostics
        );
        let actual = serde_json::to_value(parsed).unwrap();
        differences(
            case,
            "expected",
            &fixture["expected"],
            &actual,
            &mut mismatches,
        );
    }
    let allowed = gate(candidate, &mismatches, &decision_allowances()).unwrap_or_else(|errors| {
        panic!(
            "{candidate} failed the mandatory parity suite ({} mismatches):\n{errors}",
            mismatches.len()
        )
    });
    println!(
        "{candidate} mandatory parity: {} cases, {} mismatches, {allowed} covered by proposed decisions",
        cases.len(),
        mismatches.len()
    );
}

#[cfg(feature = "candidate-comrak")]
#[test]
fn comrak_passes_mandatory_parity_suite() {
    run_mandatory_suite("comrak", &marknexia_markdown::ComrakAdapter);
}

#[cfg(feature = "candidate-pulldown")]
#[test]
fn pulldown_passes_mandatory_parity_suite() {
    run_mandatory_suite("pulldown", &marknexia_markdown::PulldownAdapter);
}

#[test]
fn fixture_manifest_exactly_matches_directory_membership() {
    let expected = manifest_case_paths();
    let actual = directory_case_paths();
    assert_membership(&expected, &actual).unwrap();
    assert_eq!(expected.len(), 19);
}

#[test]
fn membership_rejects_same_count_fixture_substitution() {
    let expected = vec![
        "headings/alerts.case.json".to_owned(),
        "markdown/gfm-features.case.json".to_owned(),
    ];
    let substituted = vec![
        "headings/replacement.case.json".to_owned(),
        "markdown/gfm-features.case.json".to_owned(),
    ];
    let error = assert_membership(&expected, &substituted).unwrap_err();
    assert!(
        error.contains("missing: headings/alerts.case.json"),
        "{error}"
    );
    assert!(
        error.contains("unexpected: headings/replacement.case.json"),
        "{error}"
    );
}

#[test]
fn mismatch_report_identifies_nested_fields() {
    let mut mismatches = Vec::new();
    differences(
        "c",
        "expected",
        &serde_json::json!({"headings":[{"Text":"A"}], "links": []}),
        &serde_json::json!({"headings":[{"Text":"B"}], "extra": 1}),
        &mut mismatches,
    );
    let paths: Vec<_> = mismatches
        .iter()
        .map(|mismatch| mismatch.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "expected.headings[0].Text",
            "expected.links",
            "expected.extra"
        ]
    );
    assert_eq!(mismatches[1].actual, None);
    assert_eq!(mismatches[2].expected, None);
}

#[test]
fn every_decision_file_parses() {
    for allowance in decision_allowances() {
        assert!(
            ["comrak", "pulldown"].contains(&allowance.candidate.as_str()),
            "{allowance:?}"
        );
        assert!(
            manifest_case_paths().contains(&allowance.case),
            "{allowance:?}"
        );
    }
}

fn sample_decision(actual: &str) -> String {
    format!(
        "Status: **{PROPOSED_STATUS}**\n\n{ALLOWANCE_OPEN}\ncandidate: pulldown\ncase: headings/x.case.json\npath: expected.renderedBodyHtml\nexpected: \"<p>a</p>\\n\"\nactual: {actual}\n-->\n"
    )
}

fn sample_mismatch(actual: &str) -> Mismatch {
    Mismatch {
        case: "headings/x.case.json".to_owned(),
        path: "expected.renderedBodyHtml".to_owned(),
        expected: Some(Value::String("<p>a</p>\n".to_owned())),
        actual: Some(Value::String(actual.to_owned())),
    }
}

#[test]
fn gate_accepts_only_exact_proposed_differences() {
    let allowances = parse_allowances("d.md", &sample_decision("\"<p>b</p>\\n\"")).unwrap();
    assert_eq!(
        gate("pulldown", &[sample_mismatch("<p>b</p>\n")], &allowances),
        Ok(1)
    );
    // A changed observable difference is not covered.
    let changed = gate("pulldown", &[sample_mismatch("<p>c</p>\n")], &allowances).unwrap_err();
    assert!(
        changed.contains("no matching proposed decision"),
        "{changed}"
    );
    assert!(changed.contains("stale allowance"), "{changed}");
    // Allowances are per candidate.
    let other = gate("comrak", &[sample_mismatch("<p>b</p>\n")], &allowances).unwrap_err();
    assert!(other.contains("no matching proposed decision"), "{other}");
    // A fixed difference makes the allowance stale.
    let stale = gate("pulldown", &[], &allowances).unwrap_err();
    assert!(stale.contains("stale allowance in d.md"), "{stale}");
    assert_eq!(gate("comrak", &[], &allowances), Ok(0));
}

#[test]
fn allowances_require_proposed_status_and_complete_fields() {
    let unapproved = sample_decision("\"x\"").replace(PROPOSED_STATUS, "approved");
    assert!(
        parse_allowances("d.md", &unapproved)
            .unwrap_err()
            .contains("require status")
    );
    let incomplete = sample_decision("\"x\"").replace("path: expected.renderedBodyHtml\n", "");
    assert!(
        parse_allowances("d.md", &incomplete)
            .unwrap_err()
            .contains("missing `path`")
    );
    let absent = parse_allowances("d.md", &sample_decision("absent")).unwrap();
    assert_eq!(absent[0].actual, None);
    assert!(
        parse_allowances("d.md", "no allowances here")
            .unwrap()
            .is_empty()
    );
}
