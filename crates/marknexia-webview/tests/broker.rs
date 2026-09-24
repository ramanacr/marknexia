use marknexia_webview::broker::{
    BrokerDecision, BrokerRequest, ResourceBroker, ResourceKind, TabResourceBroker,
};

fn broker() -> TabResourceBroker {
    TabResourceBroker::for_tab(7)
}

fn request<'a>(method: &'a str, uri: &'a str, controller_tab_id: u64) -> BrokerRequest<'a> {
    BrokerRequest {
        method,
        uri,
        controller_tab_id,
        kind: ResourceKind::Image,
    }
}

#[test]
fn serves_a_valid_asset_from_its_own_tab_origin() {
    assert_eq!(
        broker().resolve(&request(
            "GET",
            "https://tab-7.marknexia.invalid/assets/figure%20one.png",
            7
        )),
        BrokerDecision::LocalAsset {
            relative_path: "figure one.png".to_owned(),
        }
    );
}

#[test]
fn document_is_served_only_at_the_exact_tab_origin() {
    let mut document = request("GET", "https://tab-7.marknexia.invalid/document", 7);
    document.kind = ResourceKind::Document;
    assert_eq!(broker().resolve(&document), BrokerDecision::Document);
    document.uri = "https://tab-7.marknexia.invalid/document?copy=1";
    assert_eq!(
        broker().resolve(&document),
        BrokerDecision::Deny { status: 403 }
    );
}

#[test]
fn only_bundled_probe_script_and_stylesheet_are_allowed() {
    let mut stylesheet = request("GET", "https://tab-7.marknexia.invalid/assets/probe.css", 7);
    stylesheet.kind = ResourceKind::Stylesheet;
    assert_eq!(
        broker().resolve(&stylesheet),
        BrokerDecision::LocalAsset {
            relative_path: "probe.css".to_owned()
        }
    );
    stylesheet.uri = "https://tab-7.marknexia.invalid/assets/other.css";
    assert_eq!(
        broker().resolve(&stylesheet),
        BrokerDecision::Deny { status: 403 }
    );

    let mut script = request("GET", "https://tab-7.marknexia.invalid/assets/probe.js", 7);
    script.kind = ResourceKind::Script;
    assert_eq!(
        broker().resolve(&script),
        BrokerDecision::LocalAsset {
            relative_path: "probe.js".to_owned()
        }
    );
}

#[test]
fn rejects_non_get_even_for_a_valid_asset() {
    assert_eq!(
        broker().resolve(&request(
            "POST",
            "https://tab-7.marknexia.invalid/assets/figure.png",
            7
        )),
        BrokerDecision::Deny { status: 405 }
    );
}

#[test]
fn rejects_cross_tab_controller_access() {
    assert_eq!(
        broker().resolve(&request(
            "GET",
            "https://tab-7.marknexia.invalid/assets/figure.png",
            8
        )),
        BrokerDecision::Deny { status: 403 }
    );
}

#[test]
fn rejects_a_different_tab_origin() {
    assert_eq!(
        broker().resolve(&request(
            "GET",
            "https://tab-8.marknexia.invalid/assets/figure.png",
            7
        )),
        BrokerDecision::Deny { status: 403 }
    );
}

#[test]
fn rejects_percent_encoded_parent_traversal() {
    assert_eq!(
        broker().resolve(&request(
            "GET",
            "https://tab-7.marknexia.invalid/assets/%2e%2e/secrets.txt",
            7
        )),
        BrokerDecision::Deny { status: 403 }
    );
}

#[test]
fn rejects_encoded_path_separator() {
    assert_eq!(
        broker().resolve(&request(
            "GET",
            "https://tab-7.marknexia.invalid/assets/dir%2fsecret.txt",
            7
        )),
        BrokerDecision::Deny { status: 403 }
    );
}

#[test]
fn rejects_windows_trailing_dot_and_space_aliases() {
    for path in ["report.png.", "report.png%20", "dir./figure.png"] {
        assert_eq!(
            broker().resolve(&request(
                "GET",
                &format!("https://tab-7.marknexia.invalid/assets/{path}"),
                7
            )),
            BrokerDecision::Deny { status: 403 },
            "{path}"
        );
    }
}

#[test]
fn rejects_reserved_windows_device_names_in_any_segment() {
    for path in ["CON.png", "images/NUL.svg", "Com1.JPG", "LPT³.gif"] {
        assert_eq!(
            broker().resolve(&request(
                "GET",
                &format!("https://tab-7.marknexia.invalid/assets/{path}"),
                7
            )),
            BrokerDecision::Deny { status: 403 },
            "{path}"
        );
    }
}

#[test]
fn rejects_win32_disallowed_filename_characters_after_decoding() {
    for path in [
        "a%3cb.png",
        "a%3eb.png",
        "a%22b.png",
        "a%7cb.png",
        "a%2ab.png",
    ] {
        assert_eq!(
            broker().resolve(&request(
                "GET",
                &format!("https://tab-7.marknexia.invalid/assets/{path}"),
                7
            )),
            BrokerDecision::Deny { status: 403 },
            "{path}"
        );
    }
}

#[test]
fn rejects_external_file_scheme() {
    assert_eq!(
        broker().resolve(&request("GET", "file:///C:/secrets.txt", 7)),
        BrokerDecision::Deny { status: 403 }
    );
}

#[test]
fn rejects_remote_network_images_by_default() {
    assert_eq!(
        broker().resolve(&request("GET", "https://example.com/figure.png", 7)),
        BrokerDecision::Deny { status: 403 }
    );
}

#[test]
fn rejects_document_owned_script_subresources() {
    let mut script = request(
        "GET",
        "https://tab-7.marknexia.invalid/assets/injected.js",
        7,
    );
    script.kind = ResourceKind::Script;
    assert_eq!(
        broker().resolve(&script),
        BrokerDecision::Deny { status: 403 }
    );
}
