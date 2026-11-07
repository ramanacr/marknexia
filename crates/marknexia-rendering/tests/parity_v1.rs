//! Mandatory rendering parity gate. Every `compat/fixtures/v1/rendering`
//! case (frozen .NET oracle output) is rendered and every expected field is
//! compared exactly, including the complete templated page. A difference
//! fails unless a `compat/decisions/*.md` file with status "approved —
//! owner decision 2026-11-06 (G0)" (or "proposed — requires Task 8
//! approval", for a record added later) records that exact candidate, case,
//! field, expected value and actual value in a `parity-allowance` block.
//! Stale allowances fail too.
//!
//! The bundled `github-markdown.css` and `bridge.js` are the .NET assets
//! plus the exact edits that `parity-asset-patch` blocks record (under the
//! same statuses): each block names the asset, a `find` text that must
//! occur exactly once and its `replace` text. The bundled asset must equal
//! the .NET asset with every recorded edit applied, and in each frozen page
//! the embedded .NET asset is replaced by that patched text before the
//! comparison, so a page may differ from .NET only by recorded edits.

use std::{collections::BTreeSet, fs, path::PathBuf};

use marknexia_core::contracts::AppTheme;
use marknexia_markdown::MarkdownEngine;
use marknexia_rendering::{
    DocumentRenderer, PageIdentity, RenderContext, RenderError, RenderLimits, Renderer,
};
use serde_json::{Value, json};

/// Status of a record added after gate G0 and not yet approved.
const PROPOSED_STATUS: &str = "proposed — requires Task 8 approval";
/// Status of a record the owner approved at gate G0 (2026-11-06).
const APPROVED_STATUS: &str = "approved — owner decision 2026-11-06 (G0)";
/// The only statuses under which a parity allowance is accepted.
/// Records approved after G0 by the owner's 2026-11-07 decision.
const APPROVED_LATER_STATUS: &str = "approved — owner decision 2026-11-07";
const ALLOWANCE_STATUSES: [&str; 3] = [APPROVED_STATUS, APPROVED_LATER_STATUS, PROPOSED_STATUS];
const ALLOWANCE_OPEN: &str = "<!-- parity-allowance";
const ASSET_PATCH_OPEN: &str = "<!-- parity-asset-patch";
/// The bundled assets that the frozen pages embed.
const ASSETS: [&str; 2] = ["github-markdown.css", "bridge.js"];
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
    let identity = PageIdentity::new(DOCUMENT_ID, NONCE).unwrap();
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
    let mut context = RenderContext::new(
        AppTheme::System,
        PageIdentity::new(DOCUMENT_ID, NONCE).unwrap(),
    );
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

/// The bodies of the comment blocks that start with `open`.
fn comment_blocks<'a>(decision: &str, text: &'a str, open: &str) -> Result<Vec<&'a str>, String> {
    let mut bodies = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let body_start = start + open.len();
        let end = rest[body_start..]
            .find("-->")
            .ok_or_else(|| format!("{decision}: unterminated {}", &open[5..]))?;
        bodies.push(&rest[body_start..body_start + end]);
        rest = &rest[body_start + end + 3..];
    }
    Ok(bodies)
}

/// The value of a `name: value` line in a block body.
fn block_field(decision: &str, body: &str, name: &str) -> Result<String, String> {
    body.lines()
        .find_map(|line| line.trim().strip_prefix(&format!("{name}:")))
        .map(|value| value.trim().to_owned())
        .ok_or_else(|| format!("{decision}: block missing `{name}`"))
}

/// Blocks count only in a record with exactly one accepted status line.
fn require_status(decision: &str, text: &str) -> Result<(), String> {
    if status_line(text).is_some_and(|status| ALLOWANCE_STATUSES.contains(&status)) {
        Ok(())
    } else {
        Err(format!(
            "{decision}: parity allowances require the line `Status: **{APPROVED_STATUS}**` or `Status: **{PROPOSED_STATUS}**`"
        ))
    }
}

fn parse_allowances(decision: &str, text: &str) -> Result<Vec<Allowance>, String> {
    let mut allowances = Vec::new();
    for body in comment_blocks(decision, text, ALLOWANCE_OPEN)? {
        let field = |name: &str| block_field(decision, body, name);
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
    if !allowances.is_empty() {
        require_status(decision, text)?;
    }
    Ok(allowances)
}

/// A recorded edit of a bundled asset relative to the .NET asset.
#[derive(Clone, Debug, PartialEq)]
struct AssetPatch {
    decision: String,
    asset: String,
    find: String,
    replace: String,
}

fn parse_asset_patches(decision: &str, text: &str) -> Result<Vec<AssetPatch>, String> {
    let mut patches = Vec::new();
    for body in comment_blocks(decision, text, ASSET_PATCH_OPEN)? {
        let text_field = |name: &str| -> Result<String, String> {
            serde_json::from_str::<String>(&block_field(decision, body, name)?)
                .map_err(|error| format!("{decision}: `{name}` is not a JSON string: {error}"))
        };
        let asset = block_field(decision, body, "asset")?;
        if !ASSETS.contains(&asset.as_str()) {
            return Err(format!("{decision}: unknown asset `{asset}`"));
        }
        let (find, replace) = (text_field("find")?, text_field("replace")?);
        if find.is_empty() || find == replace {
            return Err(format!(
                "{decision}: empty or no-op asset patch for {asset}"
            ));
        }
        patches.push(AssetPatch {
            decision: decision.to_owned(),
            asset,
            find,
            replace,
        });
    }
    if !patches.is_empty() {
        require_status(decision, text)?;
    }
    Ok(patches)
}

/// Every decision record as (file name, text), in file-name order.
fn decision_files() -> Vec<(String, String)> {
    let mut entries: Vec<_> = fs::read_dir(repository_root().join("compat/decisions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(&path).unwrap())
        })
        .collect()
}

fn decision_allowances() -> Vec<Allowance> {
    decision_files()
        .iter()
        .flat_map(|(name, text)| parse_allowances(name, text).unwrap())
        .collect()
}

fn decision_asset_patches() -> Vec<AssetPatch> {
    decision_files()
        .iter()
        .flat_map(|(name, text)| parse_asset_patches(name, text).unwrap())
        .collect()
}

/// `source` with the patches for `asset` applied in order. Each `find` must
/// occur exactly once in the text it is applied to, so a stale or ambiguous
/// patch fails.
fn apply_asset_patches(
    asset: &str,
    source: &str,
    patches: &[AssetPatch],
) -> Result<String, String> {
    let mut text = source.to_owned();
    for patch in patches.iter().filter(|patch| patch.asset == asset) {
        match text.matches(patch.find.as_str()).count() {
            1 => text = text.replacen(patch.find.as_str(), &patch.replace, 1),
            count => {
                return Err(format!(
                    "{}: {asset} patch `find` occurs {count} times, not once: {:?}",
                    patch.decision,
                    patch.find.chars().take(80).collect::<String>()
                ));
            }
        }
    }
    Ok(text)
}

/// The .NET oracle asset, newline-normalized.
fn dotnet_asset(name: &str) -> String {
    let path = repository_root()
        .join("src/Marknexia.Rendering/Assets")
        .join(name);
    // Fail loudly: a missing oracle asset must not let a check pass silently.
    let source = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{name}: cannot read the .NET oracle asset {} ({error}); the bundled copy \
             cannot be verified without it",
            path.display()
        )
    });
    source.trim_start_matches('\u{feff}').replace("\r\n", "\n")
}

/// (.NET asset, .NET asset with the recorded patches) for each changed asset.
fn patched_assets() -> Vec<(String, String)> {
    let patches = decision_asset_patches();
    ASSETS
        .iter()
        .map(|name| {
            let source = dotnet_asset(name);
            let patched = apply_asset_patches(name, &source, &patches).unwrap();
            (source, patched)
        })
        .filter(|(source, patched)| source != patched)
        .collect()
}

/// The frozen page with each embedded .NET asset replaced by its patched
/// text, or `None` when the page embeds no changed asset.
fn patch_expected_page(case: &str, page: &str, assets: &[(String, String)]) -> Option<String> {
    let mut page = page.to_owned();
    let mut changed = false;
    for (source, patched) in assets {
        match page.matches(source.as_str()).count() {
            0 => {}
            1 => {
                page = page.replacen(source.as_str(), patched, 1);
                changed = true;
            }
            count => panic!("{case}: a .NET asset is embedded {count} times"),
        }
    }
    changed.then_some(page)
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
    let assets = patched_assets();
    let mut mismatches = Vec::new();
    let mut exact = Vec::new();
    let mut asset_patched = 0;
    for case in &cases {
        let mut fixture: Value =
            serde_json::from_slice(&fs::read(fixture_root().join(case)).unwrap()).unwrap();
        assert_eq!(fixture["schemaVersion"], "marknexia-parity-v1");
        assert_eq!(fixture["area"], "rendering");
        let patched = fixture["expected"]["html"]
            .as_str()
            .and_then(|page| patch_expected_page(case, page, &assets));
        if let Some(page) = patched {
            fixture["expected"]["html"] = Value::String(page);
            asset_patched += 1;
        }
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
        "{candidate} rendering parity: {} cases, {} exact, {} mismatches, {allowed} covered by proposed decisions, {asset_patched} compared with recorded asset patches",
        cases.len(),
        exact.len(),
        mismatches.len()
    );
}

#[test]
fn pulldown_passes_rendering_parity_suite() {
    run_suite("rendering-pulldown", || marknexia_markdown::PulldownAdapter);
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
    let block = "<!-- parity-allowance\ncandidate: c\ncase: k\npath: p\nexpected: \"a\"\nactual: absent\n-->";
    let proposed = format!("Status: **{PROPOSED_STATUS}**\n{block}");
    assert_eq!(parse_allowances("d.md", &proposed).unwrap().len(), 1);
    // Any other status is rejected, including a near miss of either form.
    assert!(parse_allowances("d.md", &proposed.replace("proposed", "approved")).is_err());
    assert!(parse_allowances("d.md", &format!("Status: **approved**\n{block}")).is_err());
    assert!(parse_allowances("d.md", block).is_err());
    // Exactly one status line is required.
    let doubled = format!("Status: **{PROPOSED_STATUS}**\n{proposed}");
    assert!(parse_allowances("d.md", &doubled).is_err());
}

#[test]
fn allowances_accept_the_g0_approved_status() {
    let block = "<!-- parity-allowance\ncandidate: c\ncase: k\npath: p\nexpected: \"a\"\nactual: absent\n-->";
    let approved = format!("Status: **{APPROVED_STATUS}**\n{block}");
    assert_eq!(parse_allowances("d.md", &approved).unwrap().len(), 1);
    let near_miss = approved.replace("(G0)", "(G1)");
    assert!(
        parse_allowances("d.md", &near_miss)
            .unwrap_err()
            .contains("require the line")
    );
    let doubled = format!("Status: **{APPROVED_STATUS}**\n{approved}");
    assert!(parse_allowances("d.md", &doubled).is_err());
    let mixed = format!("Status: **{PROPOSED_STATUS}**\n{approved}");
    assert!(parse_allowances("d.md", &mixed).is_err());
}

#[test]
fn bundled_assets_match_the_dotnet_sources() {
    // The frozen fixtures embed these assets at their oracle revision; the
    // .NET working copy may use CRLF, so compare newline-normalized text.
    // The bundled copy is the .NET asset plus exactly the recorded patches.
    let patches = decision_asset_patches();
    for (name, bundled) in [
        (
            "github-markdown.css",
            include_str!("../assets/github-markdown.css"),
        ),
        ("bridge.js", include_str!("../assets/bridge.js")),
    ] {
        assert!(!bundled.contains('\r'), "{name} must be stored with LF");
        let expected = apply_asset_patches(name, &dotnet_asset(name), &patches).unwrap();
        assert_eq!(
            expected, bundled,
            "{name} drifted from the .NET asset with the recorded parity-asset-patch edits"
        );
    }
}

#[test]
fn asset_patches_apply_exactly_once_under_an_accepted_status() {
    let block = "<!-- parity-asset-patch\nasset: bridge.js\nfind: \"b\"\nreplace: \"B\"\n-->";
    let proposed = format!("Status: **{PROPOSED_STATUS}**\n{block}");
    let patches = parse_asset_patches("d.md", &proposed).unwrap();
    assert_eq!(patches.len(), 1);
    assert_eq!(
        apply_asset_patches("bridge.js", "abc", &patches).unwrap(),
        "aBc"
    );
    // Another asset is untouched.
    assert_eq!(
        apply_asset_patches("github-markdown.css", "bb", &patches).unwrap(),
        "bb"
    );
    // A stale (absent) or ambiguous (repeated) find fails.
    assert!(apply_asset_patches("bridge.js", "ac", &patches).is_err());
    assert!(apply_asset_patches("bridge.js", "abcb", &patches).is_err());
    // A patch needs an accepted status, a known asset and a real edit.
    assert!(parse_asset_patches("d.md", block).is_err());
    assert!(parse_asset_patches("d.md", &proposed.replace("bridge.js", "x.js")).is_err());
    assert!(parse_asset_patches("d.md", &proposed.replace("\"B\"", "\"b\"")).is_err());
    assert!(parse_asset_patches("d.md", &proposed.replace("\"B\"", "B")).is_err());
    // A frozen page has each changed asset replaced once.
    let assets = vec![("abc".to_owned(), "aBc".to_owned())];
    assert_eq!(
        patch_expected_page("k", "<style>abc</style>", &assets).as_deref(),
        Some("<style>aBc</style>")
    );
    assert_eq!(patch_expected_page("k", "<p></p>", &assets), None);
}
