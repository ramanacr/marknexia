//! Mandatory rendering parity gate. Every `compat/fixtures/v1/rendering`
//! case (frozen .NET oracle output) is rendered and every expected field is
//! compared exactly, including the complete templated page. A difference
//! fails unless a `compat/decisions/*.md` file with status "proposed —
//! requires Task 8 approval" records that exact candidate, case, field,
//! expected value and actual value in a `parity-allowance` block. Stale
//! allowances fail too.

use std::{collections::BTreeSet, fs, path::PathBuf};

use marknexia_core::contracts::AppTheme;
use marknexia_markdown::MarkdownEngine;
use marknexia_rendering::{
    DocumentRenderer, PageIdentity, RenderContext, RenderError, RenderLimits, Renderer,
};
use serde_json::{Value, json};

const PROPOSED_STATUS: &str = "proposed — requires Task 8 approval";
const ALLOWANCE_OPEN: &str = "<!-- parity-allowance";
const CASE_COUNT: usize = 7;

/// Distinctive template secrets, replaced by `{volatile}` exactly where the
/// .NET `ParityNormalizer` replaces the generated host and nonce.
const DOCUMENT_ID: [u8; 16] = [0x5a; 16];
const NONCE: [u8; 16] = [
    0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0x01,
];

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
    let mut paths: Vec<String> = manifest["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case.as_str().unwrap())
        .filter(|case| case.starts_with("rendering/"))
        .map(str::to_owned)
        .collect();
    paths.sort();
    paths
}

fn directory_case_paths() -> Vec<String> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(fixture_root().join("rendering")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            path.is_file() && name.ends_with(".case.json"),
            "unexpected entry {}",
            path.display()
        );
        paths.push(format!("rendering/{name}"));
    }
    paths.sort();
    paths
}

fn normalize(page: &str) -> String {
    let identity = PageIdentity::new(DOCUMENT_ID, NONCE);
    let host = identity.origin().trim_start_matches("https://").to_owned();
    let nonce = identity.nonce();
    assert_eq!(nonce.len(), 24);
    page.replace(&host, "document-{volatile}.marknexia.viewer")
        .replace(&nonce, "{volatile}")
}

fn render_case(engine: impl MarkdownEngine, fixture: &Value) -> Value {
    let input = &fixture["input"];
    let expected = &fixture["expected"];
    if expected["outcome"] == "rejected-before-read" {
        // The .NET case measures a file's length before reading it.
        let renderer = Renderer::with_engine(engine, RenderLimits::default()).unwrap();
        let size = input["inputBytes"].as_u64().unwrap();
        assert_eq!(
            input["maximumBytes"].as_u64().unwrap(),
            RenderLimits::DEFAULT_MAX_SOURCE_BYTES
        );
        return match renderer.check_source_size(size) {
            Err(RenderError::SourceTooLarge { maximum_bytes, .. }) => json!({
                "exception": "DocumentTooLargeException",
                "maximumBytes": maximum_bytes,
                "outcome": "rejected-before-read",
            }),
            other => json!({ "outcome": format!("{other:?}") }),
        };
    }
    let limits = RenderLimits {
        max_rendered_html_bytes: input["maxRenderedHtmlBytes"].as_u64().unwrap() as usize,
        max_diagram_count: input["maxDiagramCount"].as_u64().unwrap() as usize,
        max_diagram_source_bytes: input["maxDiagramSourceBytes"].as_u64().unwrap() as usize,
        ..RenderLimits::default()
    };
    let renderer = Renderer::with_engine(engine, limits).unwrap();
    assert_eq!(input["theme"], "System");
    assert_eq!(input["repositoryRoot"], ".");
    let source_path = input["sourcePath"].as_str().unwrap();
    let directory = source_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut context = RenderContext::new(AppTheme::System, PageIdentity::new(DOCUMENT_ID, NONCE));
    context.allow_remote_assets = input["allowRemoteAssets"].as_bool().unwrap();
    context.enable_diagrams = input["enableDiagrams"].as_bool().unwrap();
    context.enable_math = input["enableMath"].as_bool().unwrap();
    context.document_directory = directory.to_owned();
    match renderer.render(input["markdown"].as_str().unwrap(), &context) {
        Ok(document) => {
            let page = document.page_html();
            assert_eq!(page.len(), document.page_bytes());
            assert!(
                page.contains(document.body().as_str()),
                "page embeds the sanitized body"
            );
            json!({
                "assets": document
                    .image_references()
                    .iter()
                    .map(|image| image.as_str())
                    .collect::<Vec<_>>(),
                "html": normalize(&page),
                "outcome": "rendered",
            })
        }
        Err(RenderError::RenderedTooLarge { maximum_bytes, .. }) => json!({
            "exception": "DocumentTooLargeException",
            "maximumBytes": maximum_bytes,
            "outcome": "rejected",
        }),
        Err(other) => json!({ "outcome": format!("{other:?}") }),
    }
}

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

#[derive(Clone, Debug, PartialEq)]
struct Allowance {
    decision: String,
    candidate: String,
    case: String,
    path: String,
    expected: Option<Value>,
    actual: Option<Value>,
}

fn status_line(text: &str) -> Option<&str> {
    let mut statuses = text
        .lines()
        .filter(|line| line.trim_start().starts_with("Status:"));
    let status = statuses
        .next()?
        .trim()
        .strip_prefix("Status: **")?
        .strip_suffix("**")?;
    statuses.next().is_none().then_some(status)
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
    if !allowances.is_empty() && status_line(text) != Some(PROPOSED_STATUS) {
        return Err(format!(
            "{decision}: parity allowances require the line `Status: **{PROPOSED_STATUS}**`"
        ));
    }
    Ok(allowances)
}

fn decision_allowances() -> Vec<Allowance> {
    let mut entries: Vec<_> = fs::read_dir(repository_root().join("compat/decisions"))
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
    let Some(text) = value.as_str() else {
        return serde_json::to_string(value).unwrap();
    };
    text.to_owned()
}

fn first_difference(expected: &str, actual: &str) -> String {
    let at = expected
        .char_indices()
        .zip(actual.chars())
        .find(|((_, left), right)| left != right)
        .map_or(expected.len().min(actual.len()), |((index, _), _)| index);
    let from = expected[..at]
        .char_indices()
        .rev()
        .nth(80)
        .map_or(0, |(index, _)| index);
    format!(
        "first difference at byte {at}:\n  expected: {:?}\n  actual:   {:?}",
        &expected[from..(at + 120).min(expected.len())],
        actual.get(from..(at + 120).min(actual.len())).unwrap_or("")
    )
}

fn run_suite<E: MarkdownEngine>(candidate: &str, engine: fn() -> E) {
    let cases = manifest_case_paths();
    let on_disk = directory_case_paths();
    assert_eq!(cases, on_disk, "manifest and directory membership differ");
    assert_eq!(
        cases.iter().collect::<BTreeSet<_>>().len(),
        CASE_COUNT,
        "expected {CASE_COUNT} unique rendering cases"
    );
    let mut mismatches = Vec::new();
    let mut exact = Vec::new();
    for case in &cases {
        let fixture: Value =
            serde_json::from_slice(&fs::read(fixture_root().join(case)).unwrap()).unwrap();
        assert_eq!(fixture["schemaVersion"], "marknexia-parity-v1");
        assert_eq!(fixture["area"], "rendering");
        let actual = render_case(engine(), &fixture);
        let before = mismatches.len();
        differences(
            case,
            "expected",
            &fixture["expected"],
            &actual,
            &mut mismatches,
        );
        if mismatches.len() == before {
            exact.push(case.clone());
        } else {
            for mismatch in &mismatches[before..] {
                if let (Some(Value::String(e)), Some(Value::String(a))) =
                    (&mismatch.expected, &mismatch.actual)
                {
                    eprintln!("{case} {}: {}", mismatch.path, first_difference(e, a));
                }
            }
        }
    }
    let allowed = gate(candidate, &mismatches, &decision_allowances()).unwrap_or_else(|errors| {
        panic!(
            "{candidate} failed the rendering parity suite ({} mismatches):\n{errors}",
            mismatches.len()
        )
    });
    println!(
        "{candidate} rendering parity: {} cases, {} exact, {} mismatches, {allowed} covered by proposed decisions",
        cases.len(),
        exact.len(),
        mismatches.len()
    );
}

#[test]
fn pulldown_passes_rendering_parity_suite() {
    run_suite("rendering-pulldown", || marknexia_markdown::PulldownAdapter);
}

#[cfg(feature = "candidate-comrak")]
#[test]
fn comrak_passes_rendering_parity_suite() {
    run_suite("rendering-comrak", || marknexia_markdown::ComrakAdapter);
}

#[test]
fn gate_rejects_unlisted_and_stale_differences() {
    let mismatch = Mismatch {
        case: "rendering/x.case.json".into(),
        path: "expected.html".into(),
        expected: Some(json!("a")),
        actual: Some(json!("b")),
    };
    assert!(gate("c", std::slice::from_ref(&mismatch), &[]).is_err());
    let allowance = Allowance {
        decision: "d.md".into(),
        candidate: "c".into(),
        case: mismatch.case.clone(),
        path: mismatch.path.clone(),
        expected: mismatch.expected.clone(),
        actual: mismatch.actual.clone(),
    };
    assert_eq!(
        gate(
            "c",
            std::slice::from_ref(&mismatch),
            std::slice::from_ref(&allowance)
        ),
        Ok(1)
    );
    let stale = gate("c", &[], std::slice::from_ref(&allowance)).unwrap_err();
    assert!(stale.contains("stale allowance"), "{stale}");
    let text = "Status: **proposed — requires Task 8 approval**\n<!-- parity-allowance\ncandidate: c\ncase: k\npath: p\nexpected: \"a\"\nactual: absent\n-->";
    assert_eq!(parse_allowances("d.md", text).unwrap().len(), 1);
    assert!(parse_allowances("d.md", &text.replace("proposed", "approved")).is_err());
}

#[test]
fn bundled_assets_match_the_dotnet_sources() {
    // The frozen fixtures embed these assets at their oracle revision; the
    // .NET working copy may use CRLF, so compare newline-normalized text.
    for (name, bundled) in [
        (
            "github-markdown.css",
            include_str!("../assets/github-markdown.css"),
        ),
        ("bridge.js", include_str!("../assets/bridge.js")),
    ] {
        assert!(!bundled.contains('\r'), "{name} must be stored with LF");
        let path = repository_root()
            .join("src/Marknexia.Rendering/Assets")
            .join(name);
        if let Ok(source) = fs::read_to_string(&path) {
            let source = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
            assert_eq!(source, bundled, "{name} drifted from {}", path.display());
        }
    }
}
