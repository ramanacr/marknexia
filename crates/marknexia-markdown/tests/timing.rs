//! Release-mode parse+render timing for the dependency decision. Ignored by
//! default; run with:
//! `cargo test --release -p marknexia-markdown --all-features -- --ignored candidate_parse_render_timing --nocapture`

#![cfg(any(feature = "candidate-comrak", feature = "candidate-pulldown"))]

use marknexia_markdown::{MarkdownEngine, MarkdownOptions};
use serde_json::Value;
use std::{
    fs,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

const LARGE_TARGET_BYTES: usize = 1024 * 1024;

fn fixture_inputs() -> Vec<(String, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat/fixtures/v1");
    let mut inputs = Vec::new();
    for area in ["headings", "markdown"] {
        let mut paths: Vec<_> = fs::read_dir(root.join(area))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.to_string_lossy().ends_with(".case.json"))
            .collect();
        paths.sort();
        for path in paths {
            let case: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            inputs.push((
                format!("{area}/{}", path.file_name().unwrap().to_string_lossy()),
                case["input"]["markdown"].as_str().unwrap().to_owned(),
            ));
        }
    }
    inputs
}

/// Small: the GFM feature fixture. Large: every frozen input concatenated and
/// repeated in memory to at least 1 MiB.
fn documents() -> (String, String) {
    let inputs = fixture_inputs();
    let small = inputs
        .iter()
        .find(|(name, _)| name == "markdown/gfm-features.case.json")
        .unwrap()
        .1
        .clone();
    let mut large = String::with_capacity(LARGE_TARGET_BYTES + 64 * 1024);
    while large.len() < LARGE_TARGET_BYTES {
        for (_, input) in &inputs {
            large.push_str(input);
            large.push_str("\n\n");
        }
    }
    (small, large)
}

fn measure(engine: &dyn MarkdownEngine, source: &str, iterations: usize) -> (Duration, Duration) {
    let options = MarkdownOptions::default();
    measure_with(source, iterations, |source| {
        black_box(engine.parse(source, &options).unwrap());
    })
}

fn measure_with(source: &str, iterations: usize, run: impl Fn(&str)) -> (Duration, Duration) {
    for _ in 0..iterations.div_ceil(10) {
        run(black_box(source));
    }
    let mut samples: Vec<Duration> = (0..iterations)
        .map(|_| {
            let start = Instant::now();
            run(black_box(source));
            start.elapsed()
        })
        .collect();
    samples.sort();
    let percentile = |fraction: f64| {
        samples[((samples.len() as f64 * fraction).ceil() as usize).clamp(1, samples.len()) - 1]
    };
    (percentile(0.5), percentile(0.95))
}

fn report(name: &str, engine: &dyn MarkdownEngine, small: &str, large: &str) {
    let small_result = measure(engine, small, 2000);
    let large_result = measure(engine, large, 60);
    print_result(name, small, large, small_result, large_result);
}

fn print_result(
    name: &str,
    small: &str,
    large: &str,
    (small_median, small_p95): (Duration, Duration),
    (large_median, large_p95): (Duration, Duration),
) {
    let throughput = large.len() as f64 / (1024.0 * 1024.0) / large_median.as_secs_f64();
    println!(
        "{name}: small {} B median {:.1} us p95 {:.1} us; large {} B median {:.2} ms p95 {:.2} ms ({throughput:.1} MiB/s)",
        small.len(),
        small_median.as_secs_f64() * 1e6,
        small_p95.as_secs_f64() * 1e6,
        large.len(),
        large_median.as_secs_f64() * 1e3,
        large_p95.as_secs_f64() * 1e3,
    );
}

#[test]
#[ignore = "release-mode measurement; run explicitly"]
fn candidate_parse_render_timing() {
    let (small, large) = documents();
    assert!(large.len() >= LARGE_TARGET_BYTES);
    if cfg!(debug_assertions) {
        println!("warning: debug build; timings are not decision evidence");
    }
    #[cfg(feature = "candidate-comrak")]
    {
        report("comrak", &marknexia_markdown::ComrakAdapter, &small, &large);
        // Library-native parse+render with equivalent extensions, for the
        // overhead of the Marknexia model and Markdig-compatible writer.
        let native = |source: &str| {
            let mut options = comrak::Options::default();
            options.extension.table = true;
            options.extension.tasklist = true;
            options.extension.strikethrough = true;
            options.extension.footnotes = true;
            options.extension.autolink = true;
            options.extension.math_dollars = true;
            options.extension.highlight = true;
            options.render.r#unsafe = true;
            black_box(comrak::markdown_to_html(source, &options));
        };
        print_result(
            "comrak native renderer (baseline)",
            &small,
            &large,
            measure_with(&small, 2000, native),
            measure_with(&large, 60, native),
        );
    }
    #[cfg(feature = "candidate-pulldown")]
    {
        report(
            "pulldown",
            &marknexia_markdown::PulldownAdapter,
            &small,
            &large,
        );
        let native = |source: &str| {
            let options = pulldown_cmark::Options::ENABLE_TABLES
                | pulldown_cmark::Options::ENABLE_FOOTNOTES
                | pulldown_cmark::Options::ENABLE_TASKLISTS
                | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
                | pulldown_cmark::Options::ENABLE_MATH;
            let mut html = String::new();
            pulldown_cmark::html::push_html(
                &mut html,
                pulldown_cmark::Parser::new_ext(source, options),
            );
            black_box(html);
        };
        print_result(
            "pulldown native renderer (baseline)",
            &small,
            &large,
            measure_with(&small, 2000, native),
            measure_with(&large, 60, native),
        );
    }
}
