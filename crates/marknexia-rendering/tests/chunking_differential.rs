//! The sanitizer's chunked output must equal whole-body sanitization on real
//! renderer bodies (after the code, diagram and math passes), for the frozen
//! Markdown corpus and for generated Markdown whose raw HTML spans blocks,
//! and the rendered document must not depend on the chunking.

use std::{fs, path::PathBuf};

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{
    DocumentRenderer, PageIdentity, RenderContext, RenderError, Renderer, testing::unsanitized_body,
};
use marknexia_security::{
    ContentPolicy, PolicyLimits, RemoteImagePolicy, testing::sanitize_with_chunking,
};

fn context(remote: bool) -> RenderContext {
    let mut context = RenderContext::new(
        AppTheme::System,
        PageIdentity::new([3; 16], [5; 16]).unwrap(),
    );
    context.allow_remote_assets = remote;
    context
}

fn policy(remote: bool) -> ContentPolicy {
    let remote = if remote {
        RemoteImagePolicy::AllowHttps
    } else {
        RemoteImagePolicy::Blank
    };
    ContentPolicy::try_new(PolicyLimits::default(), remote).unwrap()
}

fn check(label: &str, source: &str) {
    let renderer = Renderer::new();
    for remote in [false, true] {
        let context = context(remote);
        let Ok(body) = unsanitized_body(&renderer, source, &context) else {
            continue;
        };
        let whole = sanitize_with_chunking(policy(remote), &body, None, 1);
        for (target, workers) in [(1, 1), (1, 4), (16, 2), (512, 8)] {
            assert_eq!(
                sanitize_with_chunking(policy(remote), &body, Some(target), workers),
                whole,
                "{label} (remote {remote}, target {target}, {workers} workers)\nbody: {body:?}"
            );
        }
        // The renderer itself (default chunking) returns the same body,
        // unless the body is white space only (.NET maps it to "").
        match renderer.render(source, &context) {
            Ok(document) if !body.chars().all(char::is_whitespace) => {
                assert_eq!(Ok(document.body().as_str().to_owned()), whole, "{label}");
            }
            Ok(document) => assert_eq!(document.body().as_str(), "", "{label}"),
            Err(RenderError::Sanitizer(error)) => assert_eq!(Err(error), whole, "{label}"),
            Err(other) => panic!("{label}: {other:?}"),
        }
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn fixture_markdown_bodies_are_identical_when_chunked() {
    let mut sources = Vec::new();
    for area in ["markdown", "headings", "rendering"] {
        for entry in fs::read_dir(repository_root().join("compat/fixtures/v1").join(area)).unwrap()
        {
            let path = entry.unwrap().path();
            let json: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            if let Some(markdown) = json["input"]["markdown"].as_str() {
                sources.push((path.display().to_string(), markdown.to_owned()));
            }
        }
    }
    assert!(sources.len() >= 20, "{}", sources.len());
    for (label, source) in &sources {
        check(label, source);
        check(label, &format!("{source}\n\n").repeat(10));
    }
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

const PIECES: &[&str] = &[
    "# Heading\n\n",
    "Paragraph with *emphasis*, `code` and [a link](a.md).\n\n",
    "- item\n- item with <b>raw bold\n\n",
    "</b> closes later\n\n",
    "<div>\n\n",
    "</div>\n\n",
    "<details>\n<summary>More</summary>\n\n",
    "</details>\n\n",
    "```cs\nvar x = 1; // c\n```\n\n",
    "```\nplain & <text>\n```\n\n",
    "```mermaid\ngraph TD\nA-->B\n```\n\n",
    "Math $a^2$ and $$b$$.\n\n",
    "| a | b |\n| - | - |\n| 1 | 2 |\n\n",
    "> quote <i>open\n\n",
    "<table>\n<tr><td>\n\n",
    "</td></tr></table>\n\n",
    "<textarea>\n\n",
    "</textarea>\n\n",
    "<pre>\n\nraw pre\n</pre>\n\n",
    "![img](https://h.test/a.png) ![local](l.png)\n\n",
    "<img src=\"//h/x\" alt=\"x\">\n\n",
    "<!-- comment\n\n",
    "-->\n\n",
    "<form>\n\n",
    "<script>alert(1)</script>\n\n",
    "text\n",
    "\n",
];

#[test]
fn generated_markdown_bodies_are_identical_when_chunked() {
    let mut rng = XorShift(0xD1B5_4A32_D192_ED03);
    let documents = if cfg!(debug_assertions) { 200 } else { 1_500 };
    for _ in 0..documents {
        let parts = 3 + rng.next() % 30;
        let source: String = (0..parts)
            .map(|_| PIECES[(rng.next() % PIECES.len() as u64) as usize])
            .collect();
        check("generated", &source);
    }
}
