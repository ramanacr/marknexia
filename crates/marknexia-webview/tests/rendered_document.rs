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
    let document = HostDocument::from_rendered(4, 1, "notes.md", &rendered, &identity).unwrap();
    let origin = identity.origin();
    assert_eq!(document.document_uri(), format!("{origin}/document"));
    assert!(document.permits_navigation(&document.document_uri()));
    assert!(!document.permits_navigation("https://tab-4.marknexia.invalid/document"));

    let page = html(&document);
    // Exactly the rendering crate's page plus one encoded title line.
    assert_eq!(
        page.replacen("  <title>notes.md</title>\n", "", 1),
        rendered.page_html()
    );
    assert!(page.contains("<head>\n  <meta charset=\"utf-8\" />\n  <title>notes.md</title>\n"));
    assert!(!page.contains("<script>alert(1)"));

    // The header policy equals the page's meta policy and names its nonce.
    let csp = document.document_csp();
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
    assert_eq!(response.content_security_policy, csp);
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
    let document =
        HostDocument::from_rendered(1, 1, "</title><script>x</script>", &rendered, &identity)
            .unwrap();
    let page = html(&document);
    assert!(page.contains("<title>&lt;/title&gt;&lt;script&gt;x&lt;/script&gt;</title>"));
}

#[test]
fn a_page_rendered_for_another_identity_is_rejected() {
    let rendered = render("# x", identity(1));
    assert_eq!(
        HostDocument::from_rendered(1, 1, "x.md", &rendered, &identity(2)),
        Err(DocumentError::IdentityMismatch)
    );
}

#[test]
fn bridge_messages_the_page_sends_are_rejected_by_the_strict_protocol() {
    let identity = identity(9);
    let rendered = render("[link](https://example.com)", identity);
    let document = HostDocument::from_rendered(2, 1, "x.md", &rendered, &identity).unwrap();
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
