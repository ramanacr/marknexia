#![no_main]

//! Arbitrary text through `Renderer::render` and `HtmlPolicy::sanitize_fragment`.
//! Invariants: no panic, output limits hold, and sanitized output is a fixed
//! point of the sanitizer (re-sanitizing changes nothing beyond two known
//! sanitizer behaviors, see `fixed_point`).

use libfuzzer_sys::fuzz_target;
use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, RenderLimits, Renderer};
use marknexia_security::{
    ContentPolicy, HtmlPolicy, MAX_HTML_OUTPUT_BYTES, PolicyLimits, RemoteImagePolicy,
};

fn policy(remote: bool) -> HtmlPolicy {
    let remote = if remote {
        RemoteImagePolicy::AllowHttps
    } else {
        RemoteImagePolicy::Deny
    };
    HtmlPolicy::new(ContentPolicy::try_new(PolicyLimits::default(), remote).unwrap())
}

/// The expected result of re-sanitizing sanitized `html`: itself, except for
/// two known sanitizer behaviors (see `tests/fuzz_invariants.rs` in
/// marknexia-rendering): white-space-only input becomes "", and one line feed
/// right after each `<pre>` is dropped by the HTML parser.
fn fixed_point(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<pre") {
        let Some(close) = rest[start..].find('>') else {
            break;
        };
        let end = start + close + 1;
        let is_pre = matches!(rest.as_bytes().get(start + 4), Some(b'>' | b' '));
        out.push_str(&rest[..end]);
        rest = &rest[end..];
        if is_pre && let Some(stripped) = rest.strip_prefix('\n') {
            rest = stripped;
        }
    }
    out.push_str(rest);
    out
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
    let html = policy(remote);
    if let Ok(clean) = html.sanitize_fragment(input) {
        assert!(clean.as_str().len() <= MAX_HTML_OUTPUT_BYTES);
        let again = html
            .sanitize_fragment(clean.as_str())
            .expect("sanitized output must be accepted again");
        assert_eq!(
            again.as_str(),
            fixed_point(clean.as_str()),
            "sanitizer is not idempotent"
        );
    }

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
        let again = policy(remote)
            .sanitize_fragment(body)
            .expect("rendered body must be accepted again");
        assert_eq!(
            again.as_str(),
            fixed_point(body),
            "rendered body is not a sanitizer fixed point"
        );
    }
});
