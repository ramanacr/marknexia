//! Hostile inputs at 50 MiB (run with `cargo test --release -p
//! marknexia-security --test scale_budgets -- --ignored --test-threads=1
//! --nocapture`).
//!
//! Every budget must still fail closed at the large sizes, and in time
//! linear in the input: each input is measured at 5 MiB and at 50 MiB, and
//! ten times the input may take at most about ten times as long (a quadratic
//! path would take a hundred times as long). The inputs that are accepted
//! since the Marknexia serializer (formerly quadratic escaping) and since
//! chunking (speculative segments that all have to be re-probed) are
//! measured the same way.

use std::time::{Duration, Instant};

use marknexia_security::{
    ContentPolicy, HtmlPolicy, PolicyLimits, RemoteImagePolicy, SanitizeError,
};

const MIB: usize = 1024 * 1024;

fn fill(prefix: &str, unit: &str, size: usize) -> String {
    let mut out = String::with_capacity(size + unit.len());
    out.push_str(prefix);
    while out.len() + unit.len() <= size {
        out.push_str(unit);
    }
    out
}

fn attribute_tag(count: usize) -> String {
    let mut tag = String::from("<p");
    for index in 0..count {
        tag.push_str(&format!(" a{index}=\"v\""));
    }
    tag.push_str(">t</p>\n");
    tag
}

type Outcome = Result<(), SanitizeError>;

struct Case {
    name: &'static str,
    input: fn(usize) -> String,
    expected: fn(&Outcome) -> bool,
}

fn too_deep(outcome: &Outcome) -> bool {
    matches!(outcome, Err(SanitizeError::NestingTooDeep { .. }))
}

fn too_complex(outcome: &Outcome) -> bool {
    *outcome == Err(SanitizeError::TooComplex)
}

fn deep_or_complex(outcome: &Outcome) -> bool {
    too_deep(outcome) || too_complex(outcome)
}

fn accepted(outcome: &Outcome) -> bool {
    outcome.is_ok()
}

const CASES: &[Case] = &[
    Case {
        name: "nested divs",
        input: |size| fill("", "<div>", size),
        expected: too_deep,
    },
    Case {
        name: "flat content at maximum depth",
        input: |size| fill(&"<div>".repeat(250), "<div></div>", size),
        expected: too_complex,
    },
    Case {
        name: "flat blocks at maximum depth, with split candidates",
        input: |size| fill(&"<div>\n".repeat(250), "<p>x</p>\n", size),
        expected: too_complex,
    },
    Case {
        name: "adoption agency <b><i><div></b>",
        input: |size| fill("", "<b><i><div></b>", size),
        expected: deep_or_complex,
    },
    Case {
        name: "reparent flood <b><p>x</b></p>",
        input: |size| fill("", "<b><p>x</b></p>\n", size),
        expected: too_complex,
    },
    Case {
        name: "misnested anchor tables",
        input: |size| fill("", "<a><table></a>", size),
        expected: too_complex,
    },
    Case {
        name: "fostered text",
        input: |size| fill("<table>", "x<br>", size),
        expected: too_complex,
    },
    Case {
        name: "fostered rows",
        input: |size| fill("", "<table><tr>x<b>y</table>", size),
        expected: too_complex,
    },
    Case {
        name: "one tag with too many attributes",
        input: |size| {
            let mut tag = String::from("<p");
            let mut index = 0;
            while tag.len() + 12 < size {
                tag.push_str(&format!(" a{index}"));
                index += 1;
            }
            tag
        },
        expected: too_complex,
    },
    Case {
        name: "256 attributes on every tag",
        input: |size| fill("", &attribute_tag(256), size),
        expected: accepted,
    },
    Case {
        name: "formerly quadratic escaping x&y",
        input: |size| fill("<p>", "x&y ", size),
        expected: accepted,
    },
    Case {
        name: "open formatting element: every segment re-probed",
        input: |size| fill("<b>\n", "<p>x</p>\n", size),
        expected: accepted,
    },
];

fn run(policy: &HtmlPolicy, input: &str) -> (Outcome, Duration) {
    let started = Instant::now();
    let outcome = policy.sanitize_fragment(input).map(|_| ());
    (outcome, started.elapsed())
}

#[test]
#[ignore = "50 MiB timing; run in release"]
fn budgets_fail_closed_in_linear_time_at_50_mib() {
    let policy = HtmlPolicy::new(
        ContentPolicy::try_new(PolicyLimits::default(), RemoteImagePolicy::Blank).unwrap(),
    );
    let mut failures = Vec::new();
    for case in CASES {
        let small = (case.input)(5 * MIB);
        let large = (case.input)(50 * MIB);
        let (small_outcome, small_time) = run(&policy, &small);
        drop(small);
        let (large_outcome, large_time) = run(&policy, &large);
        drop(large);
        let ratio = large_time.as_secs_f64() / small_time.as_secs_f64().max(0.001);
        eprintln!(
            "BUDGET {:<55} 5 MiB {:>8.3}s {:<28} | 50 MiB {:>8.3}s {:<28} | x{ratio:.1}",
            case.name,
            small_time.as_secs_f64(),
            format!("{small_outcome:?}"),
            large_time.as_secs_f64(),
            format!("{large_outcome:?}"),
        );
        if !(case.expected)(&small_outcome) || !(case.expected)(&large_outcome) {
            failures.push(format!("{}: unexpected outcome", case.name));
        }
        // Linear: ten times the input within about ten times the time (plus
        // scheduling noise); rejections that stop early may be far faster.
        if large_time > small_time * 16 + Duration::from_millis(500) {
            failures.push(format!("{}: {ratio:.1}x slower at 10x the size", case.name));
        }
        if large_time > Duration::from_secs(60) {
            failures.push(format!("{}: {large_time:?} at 50 MiB", case.name));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
