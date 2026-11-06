//! Deterministic stand-in for `fuzz/fuzz_targets/highlight.rs`: the same
//! invariants over pseudo-random token soups in every ColorCode language,
//! on every `cargo test` (libFuzzer runs need the ASan runtime).

use marknexia_rendering::testing::{
    highlight_code_with_budget, highlight_code_with_engine, highlight_language_ids,
};

const TOKENS: &[&str] = &[
    "if",
    "else",
    "for",
    "class",
    "def",
    "var",
    "let",
    "fun",
    "val",
    "type",
    "data",
    "module",
    "import",
    "SELECT",
    "Dim",
    "End",
    "True",
    "null",
    "x",
    "Foo",
    "a1",
    "_",
    "$v",
    "@x",
    "#x",
    "#region",
    "#include",
    "1",
    "42",
    "3.14",
    "0x1F",
    "1e5",
    "\"",
    "'",
    "`",
    "\"\"\"",
    "@\"",
    "\\",
    "//",
    "/*",
    "*/",
    "///",
    "#",
    "--",
    "{-",
    "-}",
    "(*",
    "*)",
    "%",
    "%{",
    "<!--",
    "-->",
    "<",
    ">",
    "</",
    "/>",
    "<b>",
    "<script>",
    "</script>",
    "<%",
    "%>",
    "<%@",
    "<?",
    "?>",
    "<![CDATA[",
    "]]>",
    "&amp;",
    "&#169;",
    "=",
    "==",
    "=>",
    "->",
    "::",
    ":",
    ";",
    ",",
    ".",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    "+",
    "-",
    "*",
    "**",
    "__",
    "|",
    "&",
    "^",
    "~",
    "?",
    "!",
    " ",
    "  ",
    "\t",
    "\n",
    "\r\n",
    "\n    ",
    "\n```",
    "\n# ",
    "\n* ",
    "é",
    "ñ",
    "\u{212a}",
    "ſ",
    "😀",
];

/// xorshift64*, so the soups are the same on every run.
struct Random(u64);

impl Random {
    fn next(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % bound
    }
}

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

fn check(code: &str, language: &str) {
    // A small budget keeps catastrophic soups fast; they fall back to plain.
    let (html, _) = highlight_code_with_budget(code, language, 1 << 22);
    let body = if let Some(plain) = html.strip_prefix("<pre><code>") {
        plain.strip_suffix("</code></pre>").expect("plain wrapper")
    } else {
        assert!(html.starts_with("<div class=\""), "{language}: {html}");
        let open = html.find("><pre>\n").expect("language wrapper") + "><pre>\n".len();
        html[open..]
            .strip_suffix("\n</pre></div>")
            .expect("language wrapper")
    };
    let opened = body.matches("<span").count();
    assert_eq!(
        opened,
        body.matches("</span>").count(),
        "{language}: {code:?}"
    );
    if !code.chars().any(|c| c > '\u{ffff}') {
        assert_eq!(decode(&strip_spans(body)), code, "{language}");
    }
    if language == "c#" {
        let (engine, _) = highlight_code_with_engine(code, language);
        if !engine.starts_with("<pre><code>") {
            assert_eq!(engine, html, "C# fast path and engine differ: {code:?}");
        }
    }
}

#[test]
fn token_soups_keep_the_highlighting_invariants() {
    let ids = highlight_language_ids();
    assert_eq!(ids.len(), 25);
    let mut random = Random(0x6d61_726b_6e65_7869);
    for round in 0..2_000 {
        let language = ids[round % ids.len()];
        let length = 1 + random.next(120);
        let code: String = (0..length)
            .map(|_| TOKENS[random.next(TOKENS.len())])
            .collect();
        check(&code, language);
    }
}
