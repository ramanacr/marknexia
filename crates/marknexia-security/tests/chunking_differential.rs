//! Chunked sanitization must be byte-identical to cleaning the whole input as
//! one fragment (compat/decisions/sanitizer-ammonia-differences.md, SAN-8).
//!
//! Every input is sanitized once as a single chunk and then again with tiny
//! chunk and segment targets, on one thread and on several, so that nearly
//! every split candidate at which the parser is at its initial state becomes
//! a chunk boundary, and most segments start speculatively in open markup.
//! The corpus is the frozen fixtures (sanitizer inputs and Markdown bodies)
//! plus thousands of generated documents whose raw HTML spans blocks.

use std::{fs, path::PathBuf};

use marknexia_security::{
    ContentPolicy, PolicyLimits, RemoteImagePolicy, SanitizeError, testing::sanitize_with_chunking,
};

fn policies() -> [ContentPolicy; 3] {
    [
        RemoteImagePolicy::Deny,
        RemoteImagePolicy::AllowHttps,
        RemoteImagePolicy::Blank,
    ]
    .map(|remote| ContentPolicy::try_new(PolicyLimits::default(), remote).unwrap())
}

/// Chunk/segment targets and worker counts compared against one chunk.
const SPLITS: &[(usize, usize)] = &[(1, 1), (1, 4), (7, 3), (64, 2), (4096, 8)];

fn check(label: &str, input: &str) -> usize {
    let mut chunked_runs = 0;
    for policy in policies() {
        let whole = sanitize_with_chunking(policy, input, None, 1);
        for &(target, workers) in SPLITS {
            let chunked = sanitize_with_chunking(policy, input, Some(target), workers);
            assert_eq!(
                chunked, whole,
                "{label}: target {target}, {workers} workers, {policy:?}\ninput: {input:?}"
            );
            chunked_runs += 1;
        }
    }
    chunked_runs
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every string value in the JSON tree whose key is one of `keys`.
fn strings_under(value: &serde_json::Value, keys: &[&str], out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                match child {
                    serde_json::Value::String(text) if keys.contains(&key.as_str()) => {
                        out.push(text.clone());
                    }
                    _ => strings_under(child, keys, out),
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                strings_under(item, keys, out);
            }
        }
        _ => {}
    }
}

#[test]
fn fixture_corpus_is_identical_when_chunked() {
    let mut inputs = Vec::new();
    for area in ["sanitizer", "markdown", "headings"] {
        let directory = repository_root().join("compat/fixtures/v1").join(area);
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let json: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let mut found = Vec::new();
            strings_under(
                &json,
                &["html", "renderedBodyHtml", "sanitizedHtml"],
                &mut found,
            );
            inputs.extend(
                found
                    .into_iter()
                    .map(|text| (path.display().to_string(), text)),
            );
        }
    }
    assert!(
        inputs.len() >= 20,
        "fixture corpus too small: {}",
        inputs.len()
    );
    for (label, input) in &inputs {
        check(label, input);
        // The same bodies repeated, so real multi-chunk documents occur.
        check(label, &format!("{input}\n").repeat(20));
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

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

/// Block-level Markdown output, well-formed, each ending in `>\n`.
const BLOCKS: &[&str] = &[
    "<p>text with <em>emphasis</em> &amp; <code>code</code></p>\n",
    "<h2 id=\"h\">Heading</h2>\n",
    "<ul>\n<li>item</li>\n<li><p>para</p>\n</li>\n</ul>\n",
    "<pre><code class=\"language-sh\">a &amp;&amp; b &gt; c\n</code></pre>\n",
    "<blockquote>\n<p>quote</p>\n</blockquote>\n",
    "<table>\n<thead>\n<tr>\n<th>h</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td style=\"text-align: right\">1</td>\n</tr>\n</tbody>\n</table>\n",
    "<p><img src=\"https://h.test/a.png\" alt=\"a\"> <img src=\"//h/b\"> <a href=\"x.md\">l</a></p>\n",
    "<hr />\n",
    "<pre>\n\nleading line feed</pre>\n",
    "<p>\u{a0}\u{a9}</p>\n",
];

/// Raw HTML that may stay open across blocks or change parser state.
const RAW: &[&str] = &[
    "<div>\n",
    "</div>\n",
    "<p>",
    "</p>\n",
    "<b>",
    "</b>",
    "<i>",
    "</i>\n",
    "<a href=\"#a\">",
    "</a>\n",
    "<nobr>",
    "<table>\n",
    "<tr>",
    "<td>",
    "</table>\n",
    "<form>\n",
    "</form>\n",
    "<textarea>\n",
    "</textarea>\n",
    "<pre>\n",
    "</pre>\n",
    "<select>",
    "</select>\n",
    "<svg>",
    "</svg>\n",
    "<math><mi>",
    "</math>\n",
    "<![CDATA[ x ]]>\n",
    "<!-- c -->\n",
    "<!--\n",
    "-->\n",
    "<template>",
    "</template>\n",
    "<x-widget>",
    "</x-widget>\n",
    "<object>",
    "</object>\n",
    "<marquee>",
    "</marquee>\n",
    "<button>",
    "</button>\n",
    "<li>",
    "<dd>",
    "<h1>",
    "</h3>\n",
    "</br>\n",
    "</p></p>\n",
    "<body a=\"1\">\n",
    "<html b=\"2\">\n",
    "<head>\n",
    "<frameset>\n",
    "<style>p{}</style>\n",
    "<noscript><b>n</b></noscript>\n",
    "<title>t</title>\n",
    "<input type=\"hidden\">\n",
    "<caption>",
    "<col>",
    "<option>",
    "<ruby><rb>",
    "<p title=\">\n\">",
    "<p class='\n'>",
    "text & more\n",
    "\n",
    "\n\n",
    "<br>\n",
    "<img src=\"http://h/z\" alt=x>\n",
    "<script>",
    "</script>\n",
    "<plaintext>",
];

#[test]
fn generated_documents_are_identical_when_chunked() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    let total = if cfg!(debug_assertions) { 400 } else { 4_000 };
    let mut documents = 0;
    let mut split_documents = 0;
    for _ in 0..total {
        let mut input = String::new();
        let parts = 4 + rng.next() % 40;
        for _ in 0..parts {
            // Mostly well-formed blocks, with raw HTML spanning them.
            if rng.next().is_multiple_of(3) {
                input.push_str(rng.pick(RAW));
            } else {
                input.push_str(rng.pick(BLOCKS));
            }
        }
        check("generated", &input);
        documents += 1;
        let chunked = sanitize_with_chunking(ContentPolicy::default(), &input, Some(1), 1);
        if chunked.is_ok() && input.contains(">\n<") {
            split_documents += 1;
        }
    }
    assert!(documents == total && split_documents > total / 4);
}

#[test]
fn budget_errors_do_not_depend_on_threads() {
    // A document that is too deep in one segment and flat elsewhere.
    let deep = format!("{}{}", "<p>a</p>\n".repeat(2_000), "<div>\n".repeat(300));
    for workers in [1, 2, 8] {
        assert_eq!(
            sanitize_with_chunking(ContentPolicy::default(), &deep, Some(4096), workers),
            Err(SanitizeError::NestingTooDeep { limit: 256 })
        );
    }
    // Misnesting spread over many segments: the document-wide reparent cap.
    let misnested = "<b><p>x</b></p>\n".repeat(20_000);
    let whole = sanitize_with_chunking(ContentPolicy::default(), &misnested, None, 1);
    assert_eq!(whole, Err(SanitizeError::TooComplex));
    for workers in [1, 3, 8] {
        assert_eq!(
            sanitize_with_chunking(ContentPolicy::default(), &misnested, Some(4096), workers),
            whole
        );
    }
}
