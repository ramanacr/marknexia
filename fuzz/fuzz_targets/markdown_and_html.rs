#![no_main]

//! Arbitrary text through `Renderer::render` and `HtmlPolicy::sanitize_fragment`.
//! Invariants: no panic, output limits hold, sanitized output is an exact
//! fixed point of the sanitizer (SAN-10, SAN-11), and chunked sanitization
//! equals one-chunk sanitization (SAN-8).
//!
//! marknexia-security needs `--cfg ammonia_unstable`. `cargo fuzz` sets
//! `RUSTFLAGS`, which replaces the workspace `.cargo/config.toml` rustflags,
//! so run it as `RUSTFLAGS="--cfg ammonia_unstable" cargo +nightly fuzz run
//! markdown_and_html` (cargo-fuzz appends its own flags to `RUSTFLAGS`).

use libfuzzer_sys::fuzz_target;
use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, RenderLimits, Renderer};
use marknexia_security::{
    ContentPolicy, HtmlPolicy, MAX_HTML_OUTPUT_BYTES, PolicyLimits, RemoteImagePolicy,
    testing::sanitize_with_chunking,
};

fn content_policy(flags: u8) -> ContentPolicy {
    let remote = match flags & 0x41 {
        0x01 | 0x41 => RemoteImagePolicy::AllowHttps,
        0x40 => RemoteImagePolicy::Deny,
        _ => RemoteImagePolicy::Blank,
    };
    ContentPolicy::try_new(PolicyLimits::default(), remote).unwrap()
}

/// The renderer's policy for a document with remote assets on or off.
fn render_policy(remote: bool) -> HtmlPolicy {
    let remote = if remote {
        RemoteImagePolicy::AllowHttps
    } else {
        RemoteImagePolicy::Blank
    };
    HtmlPolicy::new(ContentPolicy::try_new(PolicyLimits::default(), remote).unwrap())
}

fuzz_target!(|bytes: &[u8]| {
    let Some((&flags, rest)) = bytes.split_first() else {
        return;
    };
    let Ok(input) = std::str::from_utf8(rest) else {
        return;
    };
    let remote = flags & 1 != 0;

    // Raw HTML straight through the sanitizer.
    let policy = content_policy(flags);
    let html = HtmlPolicy::new(policy);
    let clean = html.sanitize_fragment(input);
    if let Ok(clean) = &clean {
        assert!(clean.as_str().len() <= MAX_HTML_OUTPUT_BYTES);
        let again = html
            .sanitize_fragment(clean.as_str())
            .expect("sanitized output must be accepted again");
        assert_eq!(&again, clean, "sanitizer is not a fixed point");
    }
    // Chunk at every possible split point: identical output and errors.
    assert_eq!(
        sanitize_with_chunking(policy, input, Some(1), 2),
        sanitize_with_chunking(policy, input, None, 1),
        "chunked sanitization differs"
    );

    // Markdown through the renderer, with small limits so edges are reached.
    let limits = RenderLimits {
        max_rendered_html_bytes: if flags & 0x10 != 0 {
            40_000
        } else {
            RenderLimits::DEFAULT_MAX_RENDERED_HTML_BYTES
        },
        max_diagram_count: usize::from((flags >> 5) & 3),
        max_diagram_source_bytes: if flags & 0x80 != 0 { 8 } else { 1024 },
        ..RenderLimits::default()
    };
    let renderer = Renderer::with_engine(marknexia_markdown::PulldownAdapter, limits).unwrap();
    let mut context = RenderContext::new(
        AppTheme::System,
        PageIdentity::new([1; 16], [2; 16]).unwrap(),
    );
    context.allow_remote_assets = remote;
    context.enable_diagrams = flags & 2 == 0;
    context.enable_math = flags & 4 == 0;
    context.document_directory = if flags & 8 != 0 {
        "docs/a b".into()
    } else {
        String::new()
    };
    if let Ok(document) = renderer.render(input, &context) {
        let body = document.body().as_str();
        assert!(body.len() <= MAX_HTML_OUTPUT_BYTES);
        assert!(document.page_bytes() <= limits.max_rendered_html_bytes);
        assert_eq!(document.page_html().len(), document.page_bytes());
        let again = render_policy(remote)
            .sanitize_fragment(body)
            .expect("rendered body must be accepted again");
        assert_eq!(
            again.as_str(),
            body,
            "rendered body is not a sanitizer fixed point"
        );
    }
});
