use marknexia_navigation::classify::{UriClassification, classify};

#[test]
fn classification_matches_the_frozen_dotnet_navigation_categories() {
    let cases = [
        (None, UriClassification::Empty),
        (Some("  "), UriClassification::Empty),
        (Some(" #résumé "), UriClassification::FragmentOnly),
        (Some("JaVaScRiPt:alert(1)"), UriClassification::Unsupported),
        (Some("data:text/html,x"), UriClassification::Unsupported),
        (Some("https://example.test/a"), UriClassification::Https),
        (Some("HTTP://example.test/a"), UriClassification::Http),
        (
            Some("mailto:team@example.test"),
            UriClassification::OtherExternal,
        ),
        (Some("/docs/api.md"), UriClassification::RepositoryRootPath),
        (
            Some("\\docs\\api.md"),
            UriClassification::RepositoryRootPath,
        ),
        (
            Some("C:\\repo\\README.md"),
            UriClassification::AbsoluteLocalPath,
        ),
        (
            Some("file:///C:/repo/README.md"),
            UriClassification::AbsoluteLocalPath,
        ),
        (Some("docs/api.md"), UriClassification::RelativePath),
    ];
    for (destination, expected) in cases {
        assert_eq!(classify(destination), expected, "{destination:?}");
    }
}

#[test]
fn unsupported_scheme_never_becomes_external_or_local() {
    for prefix in ["javascript:", "vbscript:", "data:"] {
        assert_eq!(
            classify(Some(&format!("{prefix}C:/repo/README.md"))),
            UriClassification::Unsupported
        );
    }
}
