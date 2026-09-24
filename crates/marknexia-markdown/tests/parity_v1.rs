#[cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]
use marknexia_markdown::{MarkdownEngine, MarkdownOptions};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::PathBuf};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat/fixtures/v1")
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

fn preview(value: &Value) -> String {
    let json = serde_json::to_string(value).unwrap();
    if json.chars().count() <= 120 {
        json
    } else {
        format!(
            "{}… ({} chars)",
            json.chars().take(120).collect::<String>(),
            json.chars().count()
        )
    }
}

fn differences(path: &str, expected: &Value, actual: &Value, out: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (key, value) in e {
                let child = format!("{path}.{key}");
                if let Some(found) = a.get(key) {
                    differences(&child, value, found, out);
                } else {
                    out.push(format!("{child}: missing"));
                }
            }
            for key in a.keys().filter(|key| !e.contains_key(*key)) {
                out.push(format!("{path}.{key}: unexpected"));
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            for index in 0..e.len().max(a.len()) {
                let child = format!("{path}[{index}]");
                match (e.get(index), a.get(index)) {
                    (Some(left), Some(right)) => differences(&child, left, right, out),
                    (Some(_), None) => out.push(format!("{child}: missing")),
                    (None, Some(_)) => out.push(format!("{child}: unexpected")),
                    (None, None) => unreachable!(),
                }
            }
        }
        _ if expected != actual => out.push(format!(
            "{path}: expected={} actual={}",
            preview(expected),
            preview(actual)
        )),
        _ => {}
    }
}

#[cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]
fn run_candidate(name: &str, engine: &dyn MarkdownEngine) {
    let cases = manifest_case_paths();
    assert_membership(&cases, &directory_case_paths()).unwrap();
    let mut case_count = 0;
    let mut mismatch_count = 0;
    for case in cases {
        let path = fixture_root().join(&case);
        let area = case.split_once('/').unwrap().0;
        let fixture: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(fixture["schemaVersion"], "marknexia-parity-v1");
        assert_eq!(fixture["area"], area);
        let source = fixture["input"]["markdown"].as_str().unwrap();
        let actual = engine.parse(source, &MarkdownOptions).unwrap();
        let actual = serde_json::to_value(actual).unwrap();
        let mut mismatch = Vec::new();
        differences("expected", &fixture["expected"], &actual, &mut mismatch);
        for item in &mismatch {
            println!("{name} bake-off {case} {item}");
        }
        mismatch_count += mismatch.len();
        case_count += 1;
    }
    println!(
        "{name} bake-off (not parity): {case_count} manifest cases, {mismatch_count} differing fields/paths"
    );
    assert_eq!(case_count, 19);
}

#[test]
fn mismatch_report_identifies_nested_fields() {
    let mut mismatch = Vec::new();
    differences(
        "expected",
        &serde_json::json!({"headings":[{"Text":"A"}]}),
        &serde_json::json!({"headings":[{"Text":"B"}]}),
        &mut mismatch,
    );
    assert_eq!(
        mismatch,
        ["expected.headings[0].Text: expected=\"A\" actual=\"B\""]
    );
}

#[test]
fn bake_off_manifest_exactly_matches_fixture_directory_membership() {
    let expected = manifest_case_paths();
    let actual = directory_case_paths();
    assert_membership(&expected, &actual).unwrap();
    assert_eq!(expected.len(), 19);
}

#[test]
fn bake_off_rejects_same_count_fixture_substitution() {
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

#[cfg(feature = "candidate-comrak")]
#[test]
fn comrak_bake_off_reports_frozen_corpus_differences_not_parity() {
    run_candidate("comrak", &marknexia_markdown::ComrakAdapter);
}

#[cfg(feature = "candidate-pulldown")]
#[test]
fn pulldown_bake_off_reports_frozen_corpus_differences_not_parity() {
    run_candidate("pulldown", &marknexia_markdown::PulldownAdapter);
}
