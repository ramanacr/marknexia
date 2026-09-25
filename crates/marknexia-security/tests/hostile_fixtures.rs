use marknexia_security::{
    ContentPolicy, CssPolicy, HtmlPolicy, PolicyLimits, RemoteImagePolicy, SanitizeError,
    SvgPolicy, UrlContext, UrlPolicy,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    input: FixtureInput,
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
fn all_frozen_hostile_fixtures_fail_closed_without_parser_backend() {
    let cases = [
        include_str!("../../../compat/fixtures/v1/sanitizer/unsafe-html.case.json"),
        include_str!("../../../compat/fixtures/v1/sanitizer/unsafe-uri.case.json"),
        include_str!("../../../compat/fixtures/v1/sanitizer/unsafe-css.case.json"),
        include_str!("../../../compat/fixtures/v1/sanitizer/unsafe-svg.case.json"),
    ];
    let policy = ContentPolicy::default();
    for source in cases {
        let case = fixture(source);
        let result = match case.input.mode.as_str() {
            "html" => HtmlPolicy::new(policy).sanitize_fragment(&case.input.html),
            "svg" => SvgPolicy::new(policy).sanitize(&case.input.html),
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

    let allowed = UrlPolicy::new(ContentPolicy::new(
        PolicyLimits::default(),
        RemoteImagePolicy::AllowHttps,
    ));
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
fn html_css_svg_and_url_limits_are_enforced_before_parsing() {
    let limits = PolicyLimits {
        max_html_input_bytes: 4,
        max_html_output_bytes: 4,
        max_svg_input_bytes: 4,
        max_css_input_bytes: 4,
        max_url_bytes: 4,
    };
    let policy = ContentPolicy::new(limits, RemoteImagePolicy::Deny);
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
