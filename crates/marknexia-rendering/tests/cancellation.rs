//! `Renderer::render_cancellable` stops between stages and inside the
//! sanitizer, returns `RenderError::Cancelled` with no partial output, and is
//! otherwise identical to `render`.

use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, RenderError, Renderer};

fn context() -> RenderContext {
    RenderContext::new(
        AppTheme::System,
        PageIdentity::new([4; 16], [6; 16]).unwrap(),
    )
}

fn document(bytes: usize) -> String {
    let block = "# Title\n\nSome *prose* with `code` and a [link](a.md).\n\n```cs\nvar x = 1;\n```\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n";
    block.repeat(bytes / block.len())
}

#[test]
fn a_callback_that_never_cancels_renders_the_same_document() {
    let source = document(600_000);
    let renderer = Renderer::new();
    let plain = renderer.render(&source, &context()).unwrap();
    let calls = AtomicUsize::new(0);
    let cancellable = renderer
        .render_cancellable(&source, &context(), &|| {
            calls.fetch_add(1, Ordering::Relaxed);
            false
        })
        .unwrap();
    assert_eq!(cancellable, plain);
    // Polled between stages and between sanitizer pieces and chunks.
    assert!(calls.load(Ordering::Relaxed) > 10);
}

#[test]
fn cancelling_before_or_during_a_render_returns_cancelled() {
    let renderer = Renderer::new();
    let source = document(600_000);
    assert_eq!(
        renderer.render_cancellable(&source, &context(), &|| true),
        Err(RenderError::Cancelled)
    );
    // Cancel after a number of polls, at every stage in turn.
    for after in [1, 2, 3, 4, 5, 8, 20, 60] {
        let calls = AtomicUsize::new(0);
        let result = renderer.render_cancellable(&source, &context(), &|| {
            calls.fetch_add(1, Ordering::Relaxed) >= after
        });
        assert_eq!(result, Err(RenderError::Cancelled), "after {after} polls");
    }
}

#[test]
fn a_large_render_stops_soon_after_cancellation() {
    let renderer = Renderer::new();
    let source = document(if cfg!(debug_assertions) { 4 } else { 24 } << 20);
    let cancelled = AtomicBool::new(false);
    let cancelled_at = std::sync::Mutex::new(None);
    let (result, stopped) = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            *cancelled_at.lock().unwrap() = Some(Instant::now());
            cancelled.store(true, Ordering::Relaxed);
        });
        let result =
            renderer.render_cancellable(&source, &context(), &|| cancelled.load(Ordering::Relaxed));
        (result, Instant::now())
    });
    let at = cancelled_at.lock().unwrap().expect("flag set");
    match result {
        Err(RenderError::Cancelled) => {
            let lag = stopped - at;
            // The Markdown parse is the only stage that cannot be interrupted.
            let bound = if cfg!(debug_assertions) { 60 } else { 10 };
            assert!(
                lag < Duration::from_secs(bound),
                "stopped {lag:?} after the flag"
            );
        }
        // A fast machine may finish first.
        Ok(_) => assert!(stopped <= at + Duration::from_millis(50)),
        Err(other) => panic!("{other:?}"),
    }
}

/// Highlighting polls the callback inside the regex engine, so a document
/// of catastrophic blocks stops within a fraction of its step budget.
#[test]
fn cancelling_stops_highlighting_inside_a_block() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("oracle/highlight.v1.json")).unwrap();
    let soup = oracle["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "json/soup-3")
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let source = format!("~~~~~~~~ json\n{soup}\n~~~~~~~~\n\n").repeat(50);
    let renderer = Renderer::new();
    let calls = AtomicUsize::new(0);
    let started = Instant::now();
    // The first two polls come from the stages before highlighting.
    let result = renderer.render_cancellable(&source, &context(), &|| {
        calls.fetch_add(1, Ordering::Relaxed) >= 4
    });
    assert_eq!(result, Err(RenderError::Cancelled));
    // A few polls of 65,536 steps each, far below the 134 million step base.
    let bound = Duration::from_secs(if cfg!(debug_assertions) { 20 } else { 2 });
    assert!(started.elapsed() < bound, "{:?}", started.elapsed());
}
