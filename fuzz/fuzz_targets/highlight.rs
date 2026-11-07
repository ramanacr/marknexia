#![no_main]

//! Arbitrary code through the ColorCode port (`HighlightCode`) in every
//! ColorCode language. Invariants:
//!
//! * no panic, and the regex work budget bounds every input;
//! * the output is the language wrapper or the plain `<pre><code>` fallback;
//! * highlighting only adds spans: the decoded text is the input (when the
//!   input has no astral characters, which a span may split into U+FFFD);
//! * for C#, the linear fast path and the regex engine agree whenever the
//!   engine finishes within its budget.
//!
//! The first byte picks the language; the rest is the code. The
//! deterministic stand-in is `crates/marknexia-rendering/tests/highlight_invariants.rs`.

use libfuzzer_sys::fuzz_target;
use marknexia_rendering::testing::{
    highlight_code_with_budget, highlight_code_with_engine_and_budget, highlight_language_ids,
};

fn decode(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let end = rest[amp..].find(';').expect("entity end") + amp;
        let entity = &rest[amp + 1..end];
        let c = match entity {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            _ => entity
                .strip_prefix('#')
                .and_then(|n| n.parse::<u32>().ok())
                .and_then(char::from_u32)
                .expect("numeric entity"),
        };
        out.push(c);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn strip_spans(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(open) = rest.find("<span") {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('>').expect("span end") + open;
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out.replace("</span>", "")
}

fuzz_target!(|bytes: &[u8]| {
    let Some((&selector, rest)) = bytes.split_first() else {
        return;
    };
    let Ok(code) = std::str::from_utf8(rest) else {
        return;
    };
    let ids = highlight_language_ids();
    let language = ids
        .get(usize::from(selector) % (ids.len() + 1))
        .copied()
        .unwrap_or("none");
    // A small budget keeps catastrophic soups fast; they fall back to plain.
    let (html, _) = highlight_code_with_budget(code, language, 1 << 22);
    if code.is_empty() {
        assert!(html.is_empty());
        return;
    }
    let body = if let Some(plain) = html.strip_prefix("<pre><code>") {
        plain.strip_suffix("</code></pre>").expect("plain wrapper")
    } else {
        let open = html.find("><pre>\n").expect("language wrapper") + "><pre>\n".len();
        assert!(html.starts_with("<div class=\""));
        html[open..]
            .strip_suffix("\n</pre></div>")
            .expect("language wrapper")
    };
    if !code.chars().any(|c| c > '\u{ffff}') {
        assert_eq!(decode(&strip_spans(body)), code);
    }
    if language == "c#" {
        let (engine, _) = highlight_code_with_engine_and_budget(code, language, 1 << 22);
        if !engine.starts_with("<pre><code>") {
            assert_eq!(engine, html, "C# fast path and engine differ");
        }
    }
});
