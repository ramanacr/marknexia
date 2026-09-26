//! Hostile-input properties for the parser-backed HTML/SVG/CSS sanitizer.
//!
//! Every output is checked two ways:
//! * structurally, by scanning the serialized markup (html5ever always quotes
//!   attribute values and escapes `"` inside them, and escapes `<` in text, so
//!   a quote-aware scan finds every tag and attribute), and
//! * for stability, by sanitizing the output again. A result that changes on
//!   re-parse indicates mutation-XSS potential and fails.

use marknexia_security::{
    ContentPolicy, HtmlPolicy, PolicyLimits, RemoteImagePolicy, SanitizeError, SvgPolicy,
};

const ALLOWED_TAGS: &[&str] = &[
    "a",
    "abbr",
    "acronym",
    "address",
    "area",
    "b",
    "big",
    "blockquote",
    "br",
    "button",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "dfn",
    "dir",
    "div",
    "dl",
    "dt",
    "em",
    "fieldset",
    "font",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "input",
    "ins",
    "kbd",
    "label",
    "legend",
    "li",
    "map",
    "menu",
    "ol",
    "optgroup",
    "option",
    "p",
    "pre",
    "q",
    "s",
    "samp",
    "select",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "section",
    "nav",
    "article",
    "aside",
    "header",
    "footer",
    "main",
    "figure",
    "figcaption",
    "data",
    "time",
    "mark",
    "ruby",
    "rt",
    "rp",
    "bdi",
    "wbr",
    "datalist",
    "keygen",
    "output",
    "progress",
    "meter",
    "details",
    "summary",
    "menuitem",
    "svg",
    "path",
    "g",
    "circle",
    "rect",
    "line",
    "text",
    "defs",
    "marker",
    "polyline",
    "polygon",
    "use",
];

struct Tag {
    name: String,
    attributes: Vec<(String, String)>,
}

/// Quote-aware scan of html5ever-serialized output.
fn scan(output: &str) -> Vec<Tag> {
    let bytes = output.as_bytes();
    let mut tags = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        i += 1;
        let closing = bytes.get(i) == Some(&b'/');
        if closing {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !matches!(bytes[i], b' ' | b'>' | b'/') {
            i += 1;
        }
        let name = output[start..i].to_owned();
        let mut attributes = Vec::new();
        loop {
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            if i >= bytes.len() || bytes[i] == b'>' {
                i += 1;
                break;
            }
            let attr_start = i;
            while i < bytes.len() && !matches!(bytes[i], b'=' | b' ' | b'>') {
                i += 1;
            }
            let attr = output[attr_start..i].to_owned();
            let mut value = String::new();
            if bytes.get(i) == Some(&b'=') {
                assert_eq!(
                    bytes.get(i + 1),
                    Some(&b'"'),
                    "unquoted attribute in {output}"
                );
                i += 2;
                let value_start = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                value = output[value_start..i]
                    .replace("&quot;", "\"")
                    .replace("&nbsp;", "\u{a0}")
                    .replace("&amp;", "&");
                i += 1;
            }
            attributes.push((attr, value));
        }
        if !closing {
            tags.push(Tag { name, attributes });
        }
    }
    tags
}

fn assert_inert(input: &str, output: &str) {
    for tag in scan(output) {
        assert!(
            ALLOWED_TAGS.contains(&tag.name.as_str()),
            "disallowed <{}> from {input:?}: {output}",
            tag.name
        );
        for (name, value) in &tag.attributes {
            let lower_name = name.to_ascii_lowercase();
            assert!(
                !lower_name.starts_with("on"),
                "event handler {name} from {input:?}: {output}"
            );
            assert!(
                !matches!(
                    lower_name.as_str(),
                    "srcdoc" | "formaction" | "srcset" | "ping" | "background" | "poster"
                ),
                "{name} survived from {input:?}: {output}"
            );
            let compact: String = value
                .chars()
                .filter(|c| !c.is_whitespace() && !c.is_control())
                .collect::<String>()
                .to_ascii_lowercase();
            for scheme in ["javascript:", "vbscript:", "data:", "file:", "blob:"] {
                assert!(
                    !compact.contains(scheme),
                    "{scheme} in {name} from {input:?}: {output}"
                );
            }
            if lower_name == "style" {
                for active in ["url(", "expression", "image-set", "var(", "@import", "\\"] {
                    assert!(
                        !compact.contains(active),
                        "{active} in style from {input:?}: {output}"
                    );
                }
            }
            if matches!(lower_name.as_str(), "href" | "xlink:href") && tag.name == "use" {
                assert!(
                    value.starts_with('#'),
                    "external <use> reference from {input:?}: {output}"
                );
            }
        }
    }
}

fn html(input: &str) -> String {
    let policy = HtmlPolicy::new(ContentPolicy::default());
    let output = policy
        .sanitize_fragment(input)
        .expect("bounded hostile input")
        .into_string();
    assert_inert(input, &output);
    let again = policy.sanitize_fragment(&output).unwrap().into_string();
    assert_eq!(again, output, "unstable under re-sanitization: {input:?}");
    output
}

fn svg(input: &str) -> String {
    let policy = SvgPolicy::new(ContentPolicy::default());
    let output = policy.sanitize(input).expect("bounded hostile input");
    let output = output.as_str().to_owned();
    assert_inert(input, &output);
    let again = policy.sanitize(&output).unwrap();
    assert_eq!(again.as_str(), output, "unstable SVG: {input:?}");
    output
}

fn case_variants(word: &str) -> Vec<String> {
    let lower = word.to_ascii_lowercase();
    let upper = word.to_ascii_uppercase();
    let alternating: String = lower
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i % 2 == 0 {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect();
    let inverse: String = lower
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i % 2 == 1 {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect();
    vec![lower, upper, alternating, inverse]
}

#[test]
fn event_handlers_are_removed_in_every_case_variant_on_every_element() {
    let handlers = [
        "onclick",
        "onerror",
        "onload",
        "onmouseover",
        "onfocus",
        "ontoggle",
        "onbegin",
        "onanimationstart",
        "onpointerenter",
        "onauxclick",
        "onbeforetoggle",
    ];
    let elements = [
        "<div {h}=alert(1)>x</div>",
        "<img src=x {h}=\"alert(1)\">",
        "<a href=\"#\" {h}='alert(1)'>x</a>",
        "<details open {h}=alert(1)><summary>s</summary></details>",
        "<input autofocus {h}=alert(1)>",
        "<svg {h}=alert(1)><path {h}=alert(1) d=\"M0 0\"/></svg>",
        "<button type=button {h} = alert(1)>b</button>",
        "<body {h}=alert(1)>b",
        "<span/{h}=alert(1)>s</span>",
    ];
    for handler in handlers {
        for variant in case_variants(handler) {
            for element in elements {
                let input = element.replace("{h}", &variant);
                let output = html(&input);
                assert!(
                    !output
                        .to_ascii_lowercase()
                        .contains(&handler.to_ascii_lowercase()),
                    "{input} -> {output}"
                );
            }
        }
    }
}

#[test]
fn obfuscated_script_urls_in_href_and_src_never_survive() {
    let payloads = [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        " \n\tjavascript:alert(1)",
        "&#106;avascript:alert(1)",
        "&#x6A;avascript:alert(1)",
        "&#0000106;&#0000097;vascript:alert(1)",
        "javascript&colon;alert(1)",
        "jav&#x09;ascript:alert(1)",
        "jav&#x0A;ascript:alert(1)",
        "jav&#x0D;ascript:alert(1)",
        "java\tscript:alert(1)",
        "&#x20;javascript:alert(1)",
        "\u{1}javascript:alert(1)",
        "vbscript:msgbox(1)",
        "VBScript:msgbox(1)",
        "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
        "livescript:alert(1)",
        "https:alert(1)",
        "//attacker.invalid/x",
        "\\\\attacker.invalid\\x",
        "file:///c:/windows/win.ini",
    ];
    for payload in payloads {
        for template in [
            "<a href=\"{p}\">x</a>",
            "<a href='{p}'>x</a>",
            "<a href={p}>x</a>",
            "<img src=\"{p}\">",
            "<input type=image src=\"{p}\">",
            "<blockquote cite=\"{p}\">q</blockquote>",
            "<area href=\"{p}\">",
            "<img longdesc=\"{p}\" alt=a>",
            "<svg><a href=\"{p}\"><text>t</text></a></svg>",
            "<svg><a xlink:href=\"{p}\"><text>t</text></a></svg>",
        ] {
            let input = template.replace("{p}", payload);
            let output = html(&input);
            for tag in scan(&output) {
                for (name, value) in tag.attributes {
                    if matches!(
                        name.as_str(),
                        "href" | "src" | "cite" | "longdesc" | "xlink:href"
                    ) {
                        // An unquoted value splits at whitespace, leaving a
                        // harmless relative prefix such as `java`.
                        assert!(
                            value == "#" || value.is_empty() || !value.contains(':'),
                            "{input} kept {name}={value:?}: {output}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn literal_script_urls_are_rewritten_to_hash_for_oracle_parity() {
    assert_eq!(
        html("<a href=\"javascript:alert(1)\">x</a>"),
        "<a href=\"#\">x</a>"
    );
    assert_eq!(html("<a href=\" vbscript:x\">x</a>"), "<a href=\"#\">x</a>");
    assert_eq!(html("<img src=\"javascript:alert(1)\">"), "<img src=\"#\">");
    // compat/decisions/sanitizer-ammonia-differences.md SAN-4: the .NET oracle
    // removes these attributes; the parsed decision rewrites them to "#".
    assert_eq!(
        html("<a href=\"&#106;avascript:alert(1)\">x</a>"),
        "<a href=\"#\">x</a>"
    );
    assert_eq!(
        html("<a href=javascript:alert(1)>x</a>"),
        "<a href=\"#\">x</a>"
    );
}

#[test]
fn dangerous_containers_are_removed_with_their_content() {
    let cases = [
        (
            "<iframe srcdoc=\"<script>alert(1)</script>\"></iframe>ok",
            "ok",
        ),
        (
            "<iframe src=\"https://example.test\">fallback</iframe>ok",
            "ok",
        ),
        (
            "<form action=\"https://example.test\"><input name=q></form>ok",
            "ok",
        ),
        ("<base href=\"https://attacker.invalid/\">ok", "ok"),
        (
            "<meta http-equiv=\"refresh\" content=\"0;url=javascript:alert(1)\">ok",
            "ok",
        ),
        (
            "<link rel=stylesheet href=\"https://attacker.invalid/x.css\">ok",
            "ok",
        ),
        (
            "<object data=\"x.swf\"><param name=a value=b>fb</object>ok",
            "ok",
        ),
        ("<embed src=\"x.swf\">ok", "ok"),
        ("<applet code=x>fb</applet>ok", "ok"),
        (
            "<style>*{background:url(javascript:alert(1))}</style>ok",
            "ok",
        ),
        ("<script>alert(1)</script>ok", "ok"),
        ("<video><source onerror=alert(1)></video>ok", "ok"),
        (
            "<button formaction=\"javascript:alert(1)\">b</button>",
            "<button>b</button>",
        ),
        (
            "<img srcset=\"x.png 1x, javascript:alert(1) 2x\" alt=a>",
            "<img alt=\"a\">",
        ),
        (
            "<div data-x=\"1\" data-marknexia-action=\"copy\">d</div>",
            "<div data-marknexia-action=\"copy\">d</div>",
        ),
        ("<div title=\"&{alert(1)}\">d</div>", "<div>d</div>"),
    ];
    for (input, expected) in cases {
        assert_eq!(html(input), expected, "{input}");
    }
}

#[test]
fn svg_references_scripts_and_handlers_are_neutralized() {
    let cases = [
        (
            "<svg onload=alert(1)><script>alert(1)</script><path d=\"M0 0\"/></svg>",
            "<svg><path d=\"M0 0\"></path></svg>",
        ),
        (
            "<svg><use href=\"#icon\"/></svg>",
            "<svg><use href=\"#icon\"></use></svg>",
        ),
        // SAN-5: external <use> references are removed.
        (
            "<svg><use href=\"https://attacker.invalid/sprite.svg#x\"/></svg>",
            "<svg><use></use></svg>",
        ),
        (
            "<svg><use href=\"data:image/svg+xml,&lt;svg id='x' onload='alert(1)'/&gt;#x\"/></svg>",
            "<svg><use></use></svg>",
        ),
        (
            "<svg><use xlink:href=\"sprite.svg#x\"/></svg>",
            "<svg><use></use></svg>",
        ),
        // SAN-4: script-scheme xlink:href is rewritten to "#".
        (
            "<svg><a xlink:href=\"javascript:alert(1)\"><text>t</text></a></svg>",
            "<svg><a xlink:href=\"#\"><text>t</text></a></svg>",
        ),
        (
            "<svg><animate attributeName=href values=\"javascript:alert(1)\"/><set attributeName=onmouseover to=alert(1)/></svg>",
            "<svg></svg>",
        ),
        (
            "<svg><foreignObject><img src=x onerror=alert(1)></foreignObject></svg>",
            "<svg></svg>",
        ),
        (
            "<svg><style>@import url(https://attacker.invalid/x.css)</style><g fill=\"red\"/></svg>",
            "<svg><g fill=\"red\"></g></svg>",
        ),
        (
            "<svg viewBox=\"0 0 10 10\"><marker id=m><path d=\"M0 0\"/></marker><line stroke=\"#000\" stroke-width=\"2\"/></svg>",
            "<svg viewBox=\"0 0 10 10\"><marker id=\"m\"><path d=\"M0 0\"></path></marker><line stroke=\"#000\" stroke-width=\"2\"></line></svg>",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(svg(input), expected, "{input}");
        assert_eq!(html(input), expected, "{input}");
    }
}

#[test]
fn style_attributes_reject_urls_expressions_and_active_values() {
    let cases = [
        "background:url(javascript:alert(1))",
        "background-image:url('https://attacker.invalid/x.png')",
        "background: URL(x)",
        "background: u\\72l(x)",
        "background: \\75rl(x)",
        "width: expression(alert(1))",
        "width: EXPRESSION(alert(1))",
        "width: e\\78pression(alert(1))",
        "list-style-image: image-set('x.png' 1x)",
        "behavior: url(x.htc)",
        "-moz-binding: url(x.xml#y)",
        "color: var(--x)",
        "content: attr(title)",
        "@import 'x.css'",
        "color: red; } body { background: url(x)",
        "font-family: \"x\\\n",
    ];
    for style in cases {
        let input = format!("<div style=\"{}\">t</div>", style.replace('"', "&quot;"));
        let output = html(&input);
        assert!(!output.contains("url"), "{input} -> {output}");
    }
    assert_eq!(
        html("<p style=\"COLOR:red;position:absolute;behavior:url(x)\">t</p>"),
        "<p style=\"color: red; position: absolute\">t</p>"
    );
}

#[test]
fn foreign_content_namespace_confusion_is_removed() {
    let payloads = [
        "<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>",
        "<math><mi><table><mi><mglyph><svg><mtext><textarea><path id=\"</textarea><img onerror=alert(1) src=1>\">",
        "<form><math><mtext></form><form><mglyph><style></math><img src onerror=alert(1)>",
        "<svg></p><style><a id=\"</style><img src=1 onerror=alert(1)>\">",
        "<svg><p><style><img src=x onerror=alert(1)></style></p></svg>",
        "<math><annotation-xml encoding=\"text/html\"><img src=x onerror=alert(1)></annotation-xml></math>",
        "<svg><desc><img src=x onerror=alert(1)></desc></svg>",
        "<svg><title><img src=x onerror=alert(1)></title></svg>",
        "<svg><iframe><a title=\"</iframe><img src onerror=alert(1)>\">t",
        "<select><template><style><img src=x onerror=alert(1)></style></template></select>",
        "<table><svg><td><img src=x onerror=alert(1)>",
    ];
    for payload in payloads {
        html(payload);
        svg(payload);
    }
}

#[test]
fn noscript_and_template_mutation_classics_are_inert() {
    let payloads = [
        "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\"></noscript>",
        "<noscript><style></noscript><img src=x onerror=alert(1)>",
        "<template><img src=x onerror=alert(1)></template>",
        "<template><script>alert(1)</script></template>",
        "<div><template shadowrootmode=open><img src=x onerror=alert(1)></template></div>",
        "<textarea></textarea><img src=x onerror=alert(1)></textarea>",
        "<xmp><img src=x onerror=alert(1)></xmp>",
        "<title><img src=x onerror=alert(1)></title>",
        "<!--><img src=x onerror=alert(1)>-->",
        "<!-- --!><img src=x onerror=alert(1)> -->",
        "<a title=\"x\"><!--</a><img src=x onerror=alert(1)>--></a>",
        "<![CDATA[<img src=x onerror=alert(1)>]]>",
    ];
    for payload in payloads {
        html(payload);
    }
    assert_eq!(
        html("<template><img src=x onerror=alert(1)></template>ok"),
        "ok"
    );
}

#[test]
fn nested_and_unclosed_markup_is_balanced() {
    assert_eq!(
        html("<div>unclosed<b>bold<i>it"),
        "<div>unclosed<b>bold<i>it</i></b></div>"
    );
    assert_eq!(html("<p><div>x</p></div>"), "<p></p><div>x<p></p></div>");
    assert_eq!(
        html("<a href=\"#a\"><a href=\"#b\">x</a>"),
        "<a href=\"#a\"></a><a href=\"#b\">x</a>"
    );
    assert_eq!(html("</div></b>text<"), "text&lt;");
    // SAN-9: html5ever escapes `<` and `>` in attribute values.
    assert_eq!(
        html("<img src=\"a.png\" alt=\"<b>\">"),
        "<img src=\"a.png\" alt=\"&lt;b&gt;\">"
    );
    let nested = format!("{}x{}", "<div>".repeat(200), "</div>".repeat(200));
    assert!(html(&nested).contains('x'));
    let unclosed_tags = "<b><i><u><s>".repeat(50);
    html(&unclosed_tags);
    let deep_css = format!(
        "<div style=\"width: {}1px{}\">x</div>",
        "calc(".repeat(200),
        ")".repeat(200)
    );
    assert_eq!(html(&deep_css), "<div>x</div>");
}

#[test]
fn oversized_input_and_output_fail_without_partial_output() {
    let limits = PolicyLimits {
        max_html_input_bytes: 256,
        max_html_output_bytes: 64,
        max_svg_input_bytes: 32,
        max_css_input_bytes: 16,
        max_url_bytes: 64,
    };
    let policy = ContentPolicy::try_new(limits, RemoteImagePolicy::Deny).unwrap();
    let html_policy = HtmlPolicy::new(policy);
    assert!(matches!(
        html_policy.sanitize_fragment(&"a".repeat(257)),
        Err(SanitizeError::InputTooLarge {
            kind: "HTML",
            limit: 256
        })
    ));
    // 40 input bytes expand to 200 output bytes: rejected while serializing.
    assert_eq!(
        html_policy.sanitize_fragment(&"&".repeat(40)),
        Err(SanitizeError::OutputTooLarge { limit: 64 })
    );
    assert_eq!(
        html_policy.sanitize_fragment(&"<b>x</b>".repeat(20)),
        Err(SanitizeError::OutputTooLarge { limit: 64 })
    );
    assert!(html_policy.sanitize_fragment(&"x".repeat(64)).is_ok());
    assert!(matches!(
        SvgPolicy::new(policy).sanitize(&format!("<svg>{}</svg>", "<g></g>".repeat(8))),
        Err(SanitizeError::InputTooLarge { kind: "SVG", .. })
    ));
    // A style attribute larger than the CSS budget is dropped, not truncated.
    assert_eq!(
        html_policy
            .sanitize_fragment("<p style=\"color: red; margin: 0\">t</p>")
            .unwrap()
            .as_str(),
        "<p>t</p>"
    );
    assert_eq!(
        html_policy
            .sanitize_fragment("<p style=\"color: red\">t</p>")
            .unwrap()
            .as_str(),
        "<p style=\"color: red\">t</p>"
    );
    // A URL longer than the URL budget is removed.
    let long = format!("<a href=\"https://example.test/{}\">x</a>", "a".repeat(64));
    assert_eq!(
        html_policy.sanitize_fragment(&long).unwrap().as_str(),
        "<a>x</a>"
    );
}

#[test]
fn link_and_remote_image_policies_are_independent() {
    let denied = HtmlPolicy::new(ContentPolicy::default());
    let allowed = HtmlPolicy::new(
        ContentPolicy::try_new(PolicyLimits::default(), RemoteImagePolicy::AllowHttps).unwrap(),
    );
    let input = "<a href=\"https://example.test/\">l</a><img src=\"https://example.test/i.png\" alt=\"i\"><img src=\"local.png\">";
    assert_eq!(
        denied.sanitize_fragment(input).unwrap().as_str(),
        "<a href=\"https://example.test/\">l</a><img alt=\"i\"><img src=\"local.png\">"
    );
    assert_eq!(allowed.sanitize_fragment(input).unwrap().as_str(), input);
    // The frozen remote-images-on rendering fixture.
    assert_eq!(
        allowed
            .sanitize_fragment("<p><img src=\"https://example.test/image.png\" alt=\"remote\"></p>")
            .unwrap()
            .as_str(),
        "<p><img src=\"https://example.test/image.png\" alt=\"remote\"></p>"
    );
    for value in [
        "mailto:reader@example.test",
        "tel:+1-555-0100",
        "http://example.test/",
        "#top",
        "docs/page.md",
    ] {
        let link = format!("<a href=\"{value}\">x</a>");
        assert_eq!(denied.sanitize_fragment(&link).unwrap().as_str(), link);
    }
    // SAN-3: stricter than the oracle.
    for value in [
        "/abs",
        "https://example.test:8443/",
        "https://[2001:db8::1]/",
    ] {
        let link = format!("<a href=\"{value}\">x</a>");
        assert_eq!(
            denied.sanitize_fragment(&link).unwrap().as_str(),
            "<a>x</a>"
        );
    }
    assert_eq!(
        allowed
            .sanitize_fragment("<img src=\"http://example.test/i.png\">")
            .unwrap()
            .as_str(),
        "<img>"
    );
}

#[test]
fn unknown_elements_are_unwrapped_and_known_ones_removed_with_content() {
    // compat/decisions/sanitizer-ammonia-differences.md SAN-6.
    // SAN-6: an unknown element keeps its sanitized children.
    assert_eq!(
        html("<x-widget onclick=alert(1)>text<b>b</b></x-widget>"),
        "text<b>b</b>"
    );
    assert_eq!(html("<hgroup><h1>t</h1></hgroup>ok"), "ok");
}

#[test]
fn deep_nesting_is_rejected_before_quadratic_tree_construction() {
    // compat/decisions/sanitizer-ammonia-differences.md SAN-8.
    let html_policy = HtmlPolicy::new(ContentPolicy::default());
    let svg_policy = SvgPolicy::new(ContentPolicy::default());
    let limit = marknexia_security::MAX_NESTING_DEPTH;
    for input in [
        format!("{}x{}", "<div>".repeat(20_000), "</div>".repeat(20_000)),
        "<div>".repeat(800_000),
        format!("<svg>{}</svg>", "<g>".repeat(20_000)),
        "<b><div><span>".repeat(5_000),
        format!("<table>{}", "<div>".repeat(5_000)),
        "<math><mrow>".repeat(5_000),
        "<b><i><u><s>".repeat(5_000),
        // Adoption agency: the furthest block is filled into a detached
        // formatting clone that is inserted afterwards.
        "<b><i><div></b>".repeat(300),
        "<a><b><div></a>".repeat(300),
    ] {
        let started = std::time::Instant::now();
        assert_eq!(
            html_policy.sanitize_fragment(&input),
            Err(SanitizeError::NestingTooDeep { limit })
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "depth rejection took {:?}",
            started.elapsed()
        );
    }
    assert_eq!(
        svg_policy.sanitize(&format!("<svg>{}</svg>", "<g>".repeat(1_000))),
        Err(SanitizeError::NestingTooDeep { limit })
    );
}

/// Wall-clock bound for the rejection paths; generous in debug builds, tight
/// under `cargo test --release`.
fn rejection_bound() -> std::time::Duration {
    if cfg!(debug_assertions) {
        std::time::Duration::from_secs(20)
    } else {
        std::time::Duration::from_secs(1)
    }
}

#[test]
fn adoption_agency_nesting_is_rejected_within_a_time_bound() {
    // compat/decisions/sanitizer-ammonia-differences.md SAN-8.
    let html_policy = HtmlPolicy::new(ContentPolicy::default());
    for pattern in ["<b><i><div></b>", "<a><b><div></a>"] {
        let input = pattern.repeat(20_000);
        let started = std::time::Instant::now();
        let result = html_policy.sanitize_fragment(&input);
        let elapsed = started.elapsed();
        assert!(
            matches!(
                result,
                Err(SanitizeError::NestingTooDeep { .. } | SanitizeError::TooComplex)
            ),
            "{pattern} x 20000 -> {result:?}"
        );
        assert!(
            elapsed < rejection_bound(),
            "{pattern} x 20000 took {elapsed:?}"
        );
    }
}

#[test]
fn flat_content_under_maximum_nesting_exhausts_the_work_budget() {
    // SAN-8: 250 levels of nesting followed by ~4 MiB of flat siblings.
    let html_policy = HtmlPolicy::new(ContentPolicy::default());
    let input = format!("{}{}", "<div>".repeat(250), "<div></div>".repeat(370_000));
    let started = std::time::Instant::now();
    assert_eq!(
        html_policy.sanitize_fragment(&input),
        Err(SanitizeError::TooComplex)
    );
    let elapsed = started.elapsed();
    assert!(elapsed < rejection_bound(), "flat 4 MiB took {elapsed:?}");
    // Repeated misnesting that never deepens the tree hits the reparent cap.
    assert_eq!(
        html_policy.sanitize_fragment(&"<b><p>x</b></p>".repeat(5_000)),
        Err(SanitizeError::TooComplex)
    );
}

#[test]
fn svg_paint_accepts_only_fragment_urls() {
    // SAN-7.
    assert_eq!(
        svg("<svg><rect fill=\"url(#grad)\" stroke=\"red\"/></svg>"),
        "<svg><rect fill=\"url(#grad)\" stroke=\"red\"></rect></svg>"
    );
    assert_eq!(
        svg(
            "<svg><rect fill=\"url(https://evil.example/x.svg#p)\" stroke=\"url('//evil.example/y')\"/></svg>"
        ),
        "<svg><rect></rect></svg>"
    );
}

#[test]
fn sanitized_svg_is_inline_only_and_embeds_as_a_fragment() {
    let svg = SvgPolicy::new(ContentPolicy::default())
        .sanitize("<svg><path d=\"M0 0\"/></svg><p>after</p>")
        .unwrap();
    // HTML siblings and no xmlns: this is not a standalone SVG document.
    assert_eq!(
        svg.as_str(),
        "<svg><path d=\"M0 0\"></path></svg><p>after</p>"
    );
    let fragment = svg.into_fragment();
    assert_eq!(
        HtmlPolicy::new(ContentPolicy::default())
            .sanitize_fragment(fragment.as_str())
            .unwrap(),
        fragment
    );
}

#[test]
fn whitespace_only_input_yields_empty_output() {
    assert_eq!(html(""), "");
    assert_eq!(html(" \n\t "), "");
    assert_eq!(svg("  "), "");
}
