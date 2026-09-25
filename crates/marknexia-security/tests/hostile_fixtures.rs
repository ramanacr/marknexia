use marknexia_security::{
    ContentPolicy, CssPolicy, HtmlPolicy, PolicyLimits, RemoteImagePolicy, SanitizeError,
    SvgPolicy, UrlContext, UrlPolicy,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    input: FixtureInput,
    expected: FixtureExpected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureExpected {
    sanitized_html: String,
}

#[derive(Deserialize)]
struct FixtureInput {
    html: String,
    mode: String,
}

fn fixture(source: &str) -> Fixture {
    serde_json::from_str(source).expect("frozen fixture must remain valid JSON")
}

#[test]
fn all_frozen_hostile_fixtures_preserve_expected_parity_and_fail_closed() {
    let cases = [
        include_str!("../../../compat/fixtures/v1/sanitizer/unsafe-html.case.json"),
        include_str!("../../../compat/fixtures/v1/sanitizer/unsafe-uri.case.json"),
        include_str!("../../../compat/fixtures/v1/sanitizer/unsafe-css.case.json"),
        include_str!("../../../compat/fixtures/v1/sanitizer/unsafe-svg.case.json"),
    ];
    let policy = ContentPolicy::default();
    let expected = [
        ("unsafe-html", "<a href=\"#\">bad</a>"),
        ("unsafe-uri", "<img src=\"#\">"),
        (
            "unsafe-css",
            "<div style=\"background-position: initial; background-size: initial; background-repeat: initial; background-attachment: initial; background-origin: initial; background-clip: initial; background-color: initial\">text</div>",
        ),
        ("unsafe-svg", "<svg><path d=\"M0 0\"></path></svg>"),
    ];
    for (source, (expected_name, expected_output)) in cases.into_iter().zip(expected) {
        let case = fixture(source);
        assert_eq!(case.name, expected_name);
        assert_eq!(case.expected.sanitized_html, expected_output);
        let result: Result<(), SanitizeError> = match case.input.mode.as_str() {
            "html" => HtmlPolicy::new(policy)
                .sanitize_fragment(&case.input.html)
                .map(|_| ()),
            "svg" => SvgPolicy::new(policy)
                .sanitize(&case.input.html)
                .map(|_| ()),
            mode => panic!("unexpected fixture mode {mode}"),
        };
        assert!(
            matches!(
                result,
                Err(SanitizeError::ParserUnavailable | SanitizeError::SvgParserUnavailable)
            ),
            "{} must not bypass the unavailable parser boundary",
            case.name
        );
        match expected_name {
            "unsafe-uri" => assert_eq!(
                UrlPolicy::new(policy).validate("vbscript:msgbox(1)", UrlContext::Image),
                Err(SanitizeError::UnsafeUrl)
            ),
            "unsafe-css" => assert_eq!(
                CssPolicy::new(policy).sanitize_inline_style("background:url(javascript:alert(1))"),
                Err(SanitizeError::CssParserUnavailable)
            ),
            _ => {}
        }
    }
}

#[test]
fn active_scheme_and_normalization_bypasses_are_rejected() {
    let urls = UrlPolicy::new(ContentPolicy::default());
    for value in [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        " javascript:alert(1)",
        "java\nscript:alert(1)",
        "vbscript:msgbox(1)",
        "data:text/html,<script>alert(1)</script>",
        "//attacker.invalid/x",
        "https:\\attacker.invalid/x",
    ] {
        assert_eq!(
            urls.validate(value, UrlContext::Link),
            Err(SanitizeError::UnsafeUrl)
        );
    }
}

#[test]
fn remote_images_require_explicit_https_policy() {
    let denied = UrlPolicy::new(ContentPolicy::default());
    assert_eq!(
        denied.validate("https://example.test/image.png", UrlContext::Image),
        Err(SanitizeError::UnsafeUrl)
    );

    let allowed = UrlPolicy::new(
        ContentPolicy::try_new(PolicyLimits::default(), RemoteImagePolicy::AllowHttps).unwrap(),
    );
    assert_eq!(
        allowed
            .validate("https://example.test/image.png", UrlContext::Image)
            .expect("explicit HTTPS image")
            .as_str(),
        "https://example.test/image.png"
    );
    for value in [
        "http://example.test/x",
        "https://user@example.test/x",
        "https:///x",
    ] {
        assert_eq!(
            allowed.validate(value, UrlContext::Image),
            Err(SanitizeError::UnsafeUrl)
        );
    }
}

#[test]
fn approved_external_links_do_not_depend_on_remote_image_policy() {
    let urls = UrlPolicy::new(ContentPolicy::default());
    for value in [
        "https://example.test/path",
        "http://example.test/path",
        "mailto:reader@example.test",
        "tel:+1-555-0100",
    ] {
        assert_eq!(
            urls.validate(value, UrlContext::Link).unwrap().as_str(),
            value
        );
    }
    assert_eq!(
        urls.validate("https://example.test/image.png", UrlContext::Image),
        Err(SanitizeError::UnsafeUrl)
    );
}

#[test]
fn url_forms_requiring_a_real_parser_or_compatibility_decision_fail_closed() {
    let urls = UrlPolicy::new(ContentPolicy::default());
    // The feasibility policy intentionally accepts only a conservative DNS
    // authority and simple address/number forms. Port syntax, IPv6 literals,
    // trailing-dot hosts, mailto headers, and telephone extensions remain
    // deferred until a parser-backed policy and parity decision are locked.
    for value in [
        "https://example.test:8443/path",
        "https://[2001:db8::1]/path",
        "https://example.test./path",
        "mailto:reader@example.test?subject=hello",
        "tel:+1-555-0100;ext=9",
    ] {
        assert_eq!(
            urls.validate(value, UrlContext::Link),
            Err(SanitizeError::UnsafeUrl)
        );
    }
}

#[test]
fn html_css_svg_and_url_limits_are_enforced_before_parsing() {
    let limits = PolicyLimits {
        max_html_input_bytes: 4,
        max_html_output_bytes: 4,
        max_svg_input_bytes: 4,
        max_css_input_bytes: 4,
        max_url_bytes: 4,
    };
    let policy = ContentPolicy::try_new(limits, RemoteImagePolicy::Deny).unwrap();
    assert!(matches!(
        HtmlPolicy::new(policy).sanitize_fragment("12345"),
        Err(SanitizeError::InputTooLarge { kind: "HTML", .. })
    ));
    assert!(matches!(
        SvgPolicy::new(policy).sanitize("12345"),
        Err(SanitizeError::InputTooLarge { kind: "SVG", .. })
    ));
    assert!(matches!(
        CssPolicy::new(policy).sanitize_inline_style("12345"),
        Err(SanitizeError::InputTooLarge { kind: "CSS", .. })
    ));
    assert_eq!(
        UrlPolicy::new(policy).validate("12345", UrlContext::Link),
        Err(SanitizeError::UnsafeUrl)
    );
    assert_eq!(
        HtmlPolicy::new(policy).encode_text("<<<<"),
        Err(SanitizeError::OutputTooLarge { limit: 4 })
    );
}

#[test]
fn caller_limits_cannot_raise_absolute_caps() {
    let limits = PolicyLimits {
        max_html_input_bytes: marknexia_security::MAX_HTML_INPUT_BYTES + 1,
        ..PolicyLimits::default()
    };
    assert!(matches!(
        ContentPolicy::try_new(limits, RemoteImagePolicy::Deny),
        Err(SanitizeError::LimitExceedsHardCap {
            kind: "HTML input",
            ..
        })
    ));
}

#[test]
fn plain_text_encoder_cannot_create_active_markup() {
    let encoded = HtmlPolicy::new(ContentPolicy::default())
        .encode_text("<img src=x onerror='alert(1)'>&")
        .expect("bounded text");
    assert_eq!(
        encoded.as_str(),
        "&lt;img src=x onerror=&#39;alert(1)&#39;&gt;&amp;"
    );
}

#[test]
fn css_and_svg_never_return_partially_cleaned_content_without_parsers() {
    let policy = ContentPolicy::default();
    assert_eq!(
        CssPolicy::new(policy).sanitize_inline_style("color:red"),
        Err(SanitizeError::CssParserUnavailable)
    );
    assert_eq!(
        SvgPolicy::new(policy).sanitize("<svg><path/></svg>"),
        Err(SanitizeError::SvgParserUnavailable)
    );
}
