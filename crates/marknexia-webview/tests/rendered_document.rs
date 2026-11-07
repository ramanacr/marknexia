//! A rendered Markdown page hosted without reopening raw HTML.

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, Renderer};
use marknexia_webview::{
    broker::{BrokerRequest, ResourceKind},
    policy::{DEFAULT_RESPONSE_CSP, DocumentError, HostDocument},
    protocol::MessageError,
};

fn identity(seed: u8) -> PageIdentity {
    PageIdentity::new([seed; 16], [seed.wrapping_add(1); 16]).unwrap()
}

fn render(source: &str, identity: PageIdentity) -> marknexia_rendering::RenderedDocument {
    Renderer::new()
        .render(source, &RenderContext::new(AppTheme::System, identity))
        .unwrap()
}

fn html(document: &HostDocument) -> String {
    String::from_utf8(document.html().to_vec()).unwrap()
}

#[test]
fn rendered_page_is_served_from_its_identity_origin_with_matching_policy() {
    let identity = identity(0xA5);
    let rendered = render("# Heading\n\nBody <script>alert(1)</script>", identity);
    let expected_page = rendered.page_html();
    let document =
        HostDocument::from_rendered(4, 1, "notes.md", rendered, &identity, false).unwrap();
    let origin = identity.origin();
    assert_eq!(document.document_uri(), format!("{origin}/document"));
    assert!(document.permits_navigation(&document.document_uri()));
    assert!(!document.permits_navigation("https://tab-4.marknexia.invalid/document"));

    let page = html(&document);
    // Exactly the rendering crate's page plus one encoded title line.
    assert_eq!(
        page.replacen("  <title>notes.md</title>\n", "", 1),
        expected_page
    );
    assert!(page.contains("<head>\n  <meta charset=\"utf-8\" />\n  <title>notes.md</title>\n"));
    assert!(!page.contains("<script>alert(1)"));

    // The header policy is the page's meta policy plus the header-only
    // frame-ancestors directive, and names the page nonce.
    let header = document.document_csp();
    let csp = header
        .strip_suffix("; frame-ancestors 'none'")
        .expect("header adds frame-ancestors");
    assert_ne!(csp, DEFAULT_RESPONSE_CSP);
    assert!(csp.contains(&format!("'nonce-{}'", identity.nonce())));
    assert!(csp.contains(&format!("base-uri {origin};")));
    let encoded = csp.replace('\'', "&#39;");
    assert!(page.contains(&format!("content=\"{encoded}\"")));
    assert!(page.contains(&format!("<script nonce=\"{}\">", identity.nonce())));

    let request = BrokerRequest {
        method: "GET",
        uri: &document.document_uri(),
        controller_tab_id: 4,
        kind: ResourceKind::Document,
    };
    let response = document.resolve(&request);
    assert_eq!(response.status, 200);
    assert_eq!(response.content_security_policy, header);
    // Served from the shared buffer, not a copy.
    assert!(
        response
            .shared_body
            .is_some_and(|body| std::ptr::eq(body.as_ptr(), document.html().as_ptr()))
    );
    const { assert!(marknexia_webview::policy::MAX_DOCUMENT_BYTES > 128 * 1024 * 1024) };
    assert_eq!(response.body, document.html());
    // Other origins, other tabs and every asset path fail closed.
    for (uri, tab, kind) in [
        (
            "https://tab-4.marknexia.invalid/document",
            4,
            ResourceKind::Document,
        ),
        (request.uri, 5, ResourceKind::Document),
        (
            &format!("{origin}/assets/image.png"),
            4,
            ResourceKind::Image,
        ),
        (
            "https://marknexia.assets/mermaid.min.js",
            4,
            ResourceKind::Script,
        ),
    ] {
        let denied = document.resolve(&BrokerRequest {
            method: "GET",
            uri,
            controller_tab_id: tab,
            kind,
        });
        assert_eq!(denied.status, 403, "{uri}");
        assert_eq!(denied.content_security_policy, DEFAULT_RESPONSE_CSP);
    }
}

#[test]
fn title_is_plain_text() {
    let identity = identity(3);
    let rendered = render("x", identity);
    let document = HostDocument::from_rendered(
        1,
        1,
        "</title><script>x</script>",
        rendered,
        &identity,
        false,
    )
    .unwrap();
    let page = html(&document);
    assert!(page.contains("<title>&lt;/title&gt;&lt;script&gt;x&lt;/script&gt;</title>"));
}

#[test]
fn a_page_rendered_for_another_identity_is_rejected() {
    let rendered = render("# x", identity(1));
    assert_eq!(
        HostDocument::from_rendered(1, 1, "x.md", rendered, &identity(2), false),
        Err(DocumentError::IdentityMismatch)
    );
}

#[test]
fn bridge_messages_the_page_sends_are_rejected_by_the_strict_protocol() {
    let identity = identity(9);
    let rendered = render("[link](https://example.com)", identity);
    let document = HostDocument::from_rendered(2, 1, "x.md", rendered, &identity, false).unwrap();
    let source = document.document_uri();
    // bridge.js posts untagged objects without protocol, tab or epoch.
    for json in [
        br#"{"type":"openLink","href":"https://example.com"}"#.as_slice(),
        br#"{"type":"copyText","text":"x"}"#.as_slice(),
    ] {
        assert_eq!(
            document.parse_message(json, &source),
            Err(MessageError::InvalidPayload)
        );
    }
    // A well-formed protocol message from the page's own origin is accepted.
    let ready = br#"{"type":"ready","payload":{"protocol":1,"tabId":2,"documentEpoch":1}}"#;
    assert!(document.parse_message(ready, &source).is_ok());
    assert_eq!(
        document.parse_message(ready, "https://tab-2.marknexia.invalid/document"),
        Err(MessageError::WrongOrigin)
    );
}

#[test]
fn rendered_documents_are_send() {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<marknexia_rendering::RenderedDocument>();
    assert_send::<PageIdentity>();
}

#[test]
fn a_page_rendered_with_remote_images_must_match_the_setting_and_allows_https_only() {
    let identity = identity(0x3C);
    let mut context = RenderContext::new(AppTheme::System, identity);
    context.allow_remote_assets = true;
    let renderer = Renderer::new();
    let source = "![remote](https://example.com/a.png)";
    let origin = identity.origin();

    let rendered = renderer.render(source, &context).unwrap();
    let document =
        HostDocument::from_rendered(6, 1, "remote.md", rendered, &identity, true).unwrap();
    // The frozen .NET page keeps `http: https:`; the header narrows it.
    assert!(html(&document).contains(&format!("img-src {origin} data: http: https:;")));
    assert!(
        document
            .document_csp()
            .contains(&format!("img-src {origin} data: https:;"))
    );
    assert!(!document.document_csp().contains("http:"));
    assert!(html(&document).contains("https://example.com/a.png"));

    // A page whose policy does not match the caller's setting is refused,
    // in both directions.
    let rendered = renderer.render(source, &context).unwrap();
    assert_eq!(
        HostDocument::from_rendered(6, 1, "remote.md", rendered, &identity, false).err(),
        Some(DocumentError::IdentityMismatch)
    );
    let rendered = render(source, identity);
    assert_eq!(
        HostDocument::from_rendered(6, 1, "remote.md", rendered, &identity, true).err(),
        Some(DocumentError::IdentityMismatch)
    );
    let rendered = render(source, identity);
    let document =
        HostDocument::from_rendered(6, 1, "remote.md", rendered, &identity, false).unwrap();
    assert!(
        document
            .document_csp()
            .contains(&format!("img-src {origin} data:;"))
    );
}

#[test]
fn a_page_with_a_diagram_gets_the_pinned_mermaid_runtime_and_nothing_else() {
    use marknexia_webview::runtime::{BundledRuntime, MERMAID_RUNTIME_URI, MERMAID_VERSION};
    let identity = identity(0x6D);
    let rendered = render("```mermaid\ngraph TD\nA-->B\n```\n", identity);
    assert!(rendered.has_mermaid());
    let document = HostDocument::from_rendered(9, 1, "d.md", rendered, &identity, false).unwrap();
    assert!(document.loads_mermaid_runtime());
    let page = html(&document);
    // The runtime tag carries the page nonce and follows the body; the page
    // policy allows exactly that origin for scripts.
    assert!(page.contains(&format!(
        "  </div>\n  <script src=\"{MERMAID_RUNTIME_URI}\" nonce=\"{}\"></script>\n",
        identity.nonce()
    )));
    assert!(document.document_csp().contains(&format!(
        "script-src 'nonce-{}' https://marknexia.assets;",
        identity.nonce()
    )));

    let script = |uri: &str, tab: u64, kind: ResourceKind, method: &str| {
        let response = document.resolve(&BrokerRequest {
            method,
            uri,
            controller_tab_id: tab,
            kind,
        });
        (
            response.status,
            response.content_type,
            response.body.len(),
            response.static_body.is_some(),
        )
    };
    let served = script(MERMAID_RUNTIME_URI, 9, ResourceKind::Script, "GET");
    assert_eq!(
        served,
        (
            200,
            "text/javascript; charset=utf-8",
            BundledRuntime::Mermaid.bytes().len(),
            true
        )
    );
    let response = document.resolve(&BrokerRequest {
        method: "GET",
        uri: MERMAID_RUNTIME_URI,
        controller_tab_id: 9,
        kind: ResourceKind::Script,
    });
    assert_eq!(response.content_security_policy, DEFAULT_RESPONSE_CSP);
    assert_eq!(response.body, BundledRuntime::Mermaid.bytes());
    // Every response streams the embedded bytes in place, never a copy.
    let again = document.resolve(&BrokerRequest {
        method: "GET",
        uri: MERMAID_RUNTIME_URI,
        controller_tab_id: 9,
        kind: ResourceKind::Script,
    });
    assert!(std::ptr::eq(
        response.static_body.unwrap(),
        BundledRuntime::Mermaid.bytes()
    ));
    assert!(std::ptr::eq(
        again.static_body.unwrap(),
        BundledRuntime::Mermaid.bytes()
    ));
    assert!(response.shared_body.is_none());
    assert!(String::from_utf8_lossy(&BundledRuntime::Mermaid.bytes()[..4096]).contains("mermaid"));
    let text = String::from_utf8_lossy(BundledRuntime::Mermaid.bytes());
    assert!(text.contains(&format!("\"{MERMAID_VERSION}\"")));
    assert!(
        text.contains("\r\n"),
        "served in the CRLF form .NET extracts"
    );

    // Only a GET script request from this tab, at exactly this URI.
    for (uri, tab, kind, method, status) in [
        (MERMAID_RUNTIME_URI, 9, ResourceKind::Image, "GET", 403),
        (MERMAID_RUNTIME_URI, 9, ResourceKind::Document, "GET", 403),
        (MERMAID_RUNTIME_URI, 9, ResourceKind::Other, "GET", 403),
        (MERMAID_RUNTIME_URI, 9, ResourceKind::Stylesheet, "GET", 403),
        (MERMAID_RUNTIME_URI, 10, ResourceKind::Script, "GET", 403),
        (MERMAID_RUNTIME_URI, 9, ResourceKind::Script, "POST", 405),
        (
            "https://marknexia.assets/mermaid.min.js?v=1",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://marknexia.assets/mermaid.min.js#x",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://marknexia.assets/mermaid.js",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://marknexia.assets/",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://marknexia.assets/bridge.js",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "http://marknexia.assets/mermaid.min.js",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://marknexia.assets.evil/mermaid.min.js",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://marknexia.assets:444/mermaid.min.js",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://user@marknexia.assets/mermaid.min.js",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://marknexia.assets/MERMAID.MIN.JS",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://marknexia.assets/%6dermaid.min.js",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://marknexia.assets//mermaid.min.js",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
        (
            "https://cdn.jsdelivr.net/npm/mermaid@10.9.1/dist/mermaid.min.js",
            9,
            ResourceKind::Script,
            "GET",
            403,
        ),
    ] {
        assert_eq!(
            script(uri, tab, kind, method).0,
            status,
            "{uri} {kind:?} {method} tab {tab}"
        );
    }
}

#[test]
fn a_page_without_a_diagram_never_gets_the_runtime() {
    use marknexia_webview::runtime::MERMAID_RUNTIME_URI;
    for source in [
        "# No diagrams\n",
        // Diagrams over the limits render as source fallbacks without the
        // runtime (.NET `hasMermaid` stays false).
        &format!("```mermaid\n{}\n```\n", "a".repeat(1024 * 1024 + 1)),
        // Raw HTML cannot add the runtime tag: the sanitizer removes it.
        "<script src=\"https://marknexia.assets/mermaid.min.js\"></script>\n",
    ] {
        let identity = identity(0x41);
        let rendered = render(source, identity);
        assert!(!rendered.has_mermaid());
        let document =
            HostDocument::from_rendered(3, 1, "x.md", rendered, &identity, false).unwrap();
        assert!(!document.loads_mermaid_runtime());
        assert!(!html(&document).contains(MERMAID_RUNTIME_URI));
        let response = document.resolve(&BrokerRequest {
            method: "GET",
            uri: MERMAID_RUNTIME_URI,
            controller_tab_id: 3,
            kind: ResourceKind::Script,
        });
        assert_eq!(response.status, 403);
    }
    // Diagrams turned off: fallbacks, no runtime.
    let identity = identity(0x42);
    let mut context = RenderContext::new(AppTheme::System, identity);
    context.enable_diagrams = false;
    let rendered = Renderer::new()
        .render("```mermaid\ngraph TD\nA-->B\n```\n", &context)
        .unwrap();
    assert!(!rendered.has_mermaid());
    let document = HostDocument::from_rendered(3, 1, "x.md", rendered, &identity, false).unwrap();
    assert!(!document.loads_mermaid_runtime());
    assert!(html(&document).contains("diagram rendering is disabled for this document"));
}

#[test]
fn trusted_and_generated_documents_never_get_the_runtime() {
    use marknexia_webview::runtime::MERMAID_RUNTIME_URI;
    let document = marknexia_webview::probe::document(7, 1);
    assert!(!document.loads_mermaid_runtime());
    let response = document.resolve(&BrokerRequest {
        method: "GET",
        uri: MERMAID_RUNTIME_URI,
        controller_tab_id: 7,
        kind: ResourceKind::Script,
    });
    assert_eq!(response.status, 403);
}

#[test]
fn a_page_with_a_text_diagram_is_hosted_without_the_mermaid_runtime() {
    // Rust edition only (compat/decisions/text-diagrams.md, TDG-7): the
    // stylesheet grows inside the template's `<style>` element and the page
    // still passes the host's identity and policy checks.
    let identity = identity(0x51);
    let rendered = render("```diagram\na --> b\n|\nv\nc\n```\n", identity);
    assert_eq!(rendered.text_diagrams(), 1);
    assert!(!rendered.has_mermaid());
    let document = HostDocument::from_rendered(3, 1, "x.md", rendered, &identity, false).unwrap();
    assert!(!document.loads_mermaid_runtime());
    let page = html(&document);
    let style_end = page.find("\n  </style>\n</head>\n<body>\n").unwrap();
    assert!(page[..style_end].contains("/* Text diagrams (Rust edition, TDG-7) */"));
    assert!(page[style_end..].contains("<svg class=\"marknexia-text-diagram-svg\""));
}
