//! Regression tests for hostile Markdown (review of d8f0767):
//! deep nesting must not overflow a 1 MiB stack (the Windows main-thread
//! default), anchor and alert emulation must stay linear, and rendered output
//! must stay within `MarkdownOptions::max_rendered_body_bytes`.

#![cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]

use marknexia_core::contracts::DiagnosticSeverity;
use marknexia_markdown::{MarkdownEngine, MarkdownOptions};
use std::time::{Duration, Instant};

const ONE_MIB: usize = 1024 * 1024;

type Engine = &'static (dyn MarkdownEngine + Sync);

fn engines() -> Vec<(&'static str, Engine)> {
    [
        #[cfg(feature = "candidate-comrak")]
        ("comrak", &marknexia_markdown::ComrakAdapter as Engine),
        #[cfg(feature = "candidate-pulldown")]
        ("pulldown", &marknexia_markdown::PulldownAdapter as Engine),
    ]
    .into_iter()
    .collect()
}

/// Runs `body` on a thread with a 1 MiB stack. A stack overflow aborts the
/// whole test process, which fails the run.
fn on_one_mib_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(ONE_MIB)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
}

fn nested(open: &str, middle: &str, close: &str, depth: usize) -> String {
    format!("{}{middle}{}", open.repeat(depth), close.repeat(depth))
}

/// (name, input, expect a flattening diagnostic)
fn deep_inputs() -> Vec<(&'static str, String, bool)> {
    vec![
        ("quotes", format!("{}x\n", ">".repeat(20_000)), true),
        ("emphasis", nested("*a ", "x", " b*", 20_000), true),
        ("strong", nested("**", "x", "**", 20_000), false),
        ("strikethrough", nested("~~a ", "x", " a~~", 20_000), false),
        ("mark", nested("==a ", "x", " a==", 20_000), false),
        ("images", nested("![", "x", "](u)", 20_000), false),
        ("links", nested("[", "x", "](u)", 20_000), false),
        ("inline-lists", format!("{}x\n", "- ".repeat(20_000)), true),
        (
            "quoted-lists",
            format!("{}x\n", "> - ".repeat(10_000)),
            true,
        ),
        (
            "indented-lists",
            (0..400)
                .map(|depth| format!("{}- x\n", "  ".repeat(depth)))
                .collect(),
            true,
        ),
        (
            "footnote-quotes",
            format!("x[^1]\n\n[^1]: {}y\n", ">".repeat(20_000)),
            true,
        ),
        (
            "ordered-in-quotes",
            format!("{}1. x\n", "> 1. ".repeat(5_000)),
            true,
        ),
    ]
}

#[test]
fn deep_nesting_is_flattened_within_a_one_mib_stack() {
    for (name, engine) in engines() {
        for (case, source, expect_diagnostic) in deep_inputs() {
            on_one_mib_stack(move || {
                let parsed = engine
                    .parse(&source, &MarkdownOptions::default())
                    .unwrap_or_else(|errors| panic!("{name} {case}: {errors:?}"));
                assert!(!parsed.rendered_body_html.is_empty(), "{name} {case}");
                let flattened = parsed.diagnostics.iter().any(|diagnostic| {
                    diagnostic.severity == DiagnosticSeverity::Warning
                        && diagnostic.message.contains("nesting deeper than")
                });
                if expect_diagnostic {
                    assert!(flattened, "{name} {case}: {:?}", parsed.diagnostics);
                }
                // Dropping the model and output also happens on this stack.
                drop(parsed);
            });
        }
    }
}

#[test]
fn shallow_nesting_is_not_flattened() {
    for (name, engine) in engines() {
        let source = format!("{}x\n\n{}\n", "> ".repeat(8), nested("*a ", "x", " b*", 8));
        let parsed = engine.parse(&source, &MarkdownOptions::default()).unwrap();
        assert!(
            parsed.diagnostics.is_empty(),
            "{name}: {:?}",
            parsed.diagnostics
        );
        assert_eq!(
            parsed.rendered_body_html.matches("<blockquote>").count(),
            8,
            "{name}"
        );
        assert_eq!(
            parsed.rendered_body_html.matches("<em>").count(),
            8,
            "{name}"
        );
    }
}

/// Linear work on 1 MiB finishes far below these bounds; the quadratic and
/// cubic scans this guards against took minutes.
fn time_bound() -> Duration {
    if cfg!(debug_assertions) {
        Duration::from_secs(15)
    } else {
        Duration::from_secs(1)
    }
}

fn repeat_to(unit: &str, prefix: &str, suffix: &str) -> String {
    let mut text = String::with_capacity(ONE_MIB + unit.len() + suffix.len());
    text.push_str(prefix);
    while text.len() < ONE_MIB {
        text.push_str(unit);
    }
    text.push_str(suffix);
    text
}

fn linear_inputs() -> Vec<(&'static str, String)> {
    vec![
        // Review C2: every `<a ` has no later `>` (was 437 s at 128 KiB).
        ("anchors-no-close", repeat_to("<a id='x' ", "<div>\n", "")),
        // Every `<a ` shares one final `>`.
        ("anchors-shared-close", repeat_to("<a ", "<div>\n", ">")),
        (
            "anchors-shared-close-keys",
            repeat_to("<a id=x ", "<div>\n", ">"),
        ),
        ("anchors-valid", repeat_to("<a id='x'></a>", "<div>\n", "")),
        // Review I1: every candidate lacks `</blockquote>` (was 79 s at 1 MiB).
        (
            "alerts-unclosed",
            repeat_to("<blockquote><p>[!NOTE]</p>", "", ""),
        ),
        (
            "alerts-unclosed-paragraph",
            repeat_to("<blockquote><p>[!NOTE]", "", ""),
        ),
        ("alerts-valid", repeat_to("> [!NOTE]\n> x\n\n", "", "")),
    ]
}

#[test]
fn anchor_and_alert_emulation_is_linear_on_one_mib() {
    for (name, engine) in engines() {
        for (case, source) in linear_inputs() {
            let start = Instant::now();
            let parsed = engine
                .parse(&source, &MarkdownOptions::default())
                .unwrap_or_else(|errors| panic!("{name} {case}: {errors:?}"));
            let elapsed = start.elapsed();
            println!("{name} {case}: {} bytes in {elapsed:?}", source.len());
            assert!(elapsed < time_bound(), "{name} {case}: {elapsed:?}");
            drop(parsed);
        }
    }
}

#[test]
fn anchors_extracted_from_hostile_html_match_regex_semantics() {
    for (name, engine) in engines() {
        let parsed = engine
            .parse(
                "<div>\n<a id='x' <a id='y'> <a name=\"z\" id=\"w\"> <a >id='no'>\n</div>\n",
                &MarkdownOptions::default(),
            )
            .unwrap();
        let ids: Vec<_> = parsed
            .custom_anchors
            .iter()
            .map(|anchor| anchor.id.as_str())
            .collect();
        assert_eq!(ids, ["y", "w"], "{name}");
    }
}

#[test]
fn rendered_output_limit_rejects_like_dotnet() {
    for (name, engine) in engines() {
        // `compat/fixtures/v1/rendering/rendered-output-limit.case.json`:
        // "# bounded output" with a 1-byte limit is rejected.
        let tiny = MarkdownOptions {
            max_rendered_body_bytes: 1,
            ..MarkdownOptions::default()
        };
        let errors = engine.parse("# bounded output", &tiny).unwrap_err();
        assert_eq!(errors.len(), 1, "{name}");
        assert_eq!(errors[0].severity, DiagnosticSeverity::Error, "{name}");
        assert!(errors[0].message.contains("1 byte limit"), "{name}");

        // Alerts amplify ~12 source bytes into ~600 output bytes; the limit
        // stops rendering instead of allocating the whole expansion.
        let limited = MarkdownOptions {
            max_rendered_body_bytes: 64 * 1024,
            ..MarkdownOptions::default()
        };
        let alerts = "> [!NOTE]\n\n".repeat(20_000);
        assert!(engine.parse(&alerts, &limited).is_err(), "{name}");
        let footnotes = format!("{}\n\n[^1]: x\n", "[^1]".repeat(50_000));
        assert!(engine.parse(&footnotes, &limited).is_err(), "{name}");

        let fits = MarkdownOptions {
            max_rendered_body_bytes: 64,
            ..MarkdownOptions::default()
        };
        assert!(engine.parse("# bounded output", &fits).is_ok(), "{name}");
    }
}
