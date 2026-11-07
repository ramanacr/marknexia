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

#[test]
fn the_mermaid_runtime_is_served_only_when_enabled_and_only_as_a_script() {
    use marknexia_webview::broker::{BundledRuntime, MERMAID_RUNTIME_URI};
    let mut script = request("GET", MERMAID_RUNTIME_URI, 7);
    script.kind = ResourceKind::Script;
    assert_eq!(
        broker().resolve(&script),
        BrokerDecision::Deny { status: 403 }
    );
    let enabled = broker().with_mermaid_runtime(true);
    assert_eq!(
        enabled.resolve(&script),
        BrokerDecision::BundledRuntime(BundledRuntime::Mermaid)
    );
    for kind in [
        ResourceKind::Document,
        ResourceKind::Image,
        ResourceKind::Stylesheet,
        ResourceKind::Other,
    ] {
        let mut other = script;
        other.kind = kind;
        assert_eq!(
            enabled.resolve(&other),
            BrokerDecision::Deny { status: 403 },
            "{kind:?}"
        );
    }
    let mut cross_tab = script;
    cross_tab.controller_tab_id = 8;
    assert_eq!(
        enabled.resolve(&cross_tab),
        BrokerDecision::Deny { status: 403 }
    );
    let mut post = script;
    post.method = "POST";
    assert_eq!(enabled.resolve(&post), BrokerDecision::Deny { status: 405 });
    // The runtime host serves nothing else, and the tab origin cannot alias it.
    for uri in [
        "https://marknexia.assets/mermaid.min.js?x",
        "https://marknexia.assets/probe.js",
        "https://tab-7.marknexia.invalid/assets/mermaid.min.js",
        "https://tab-7.marknexia.invalid/mermaid.min.js",
    ] {
        let mut other = script;
        other.uri = uri;
        assert_eq!(
            enabled.resolve(&other),
            BrokerDecision::Deny { status: 403 },
            "{uri}"
        );
    }
}

#[test]
fn every_single_edit_of_the_runtime_uri_is_denied() {
    use marknexia_webview::broker::MERMAID_RUNTIME_URI;
    let enabled = broker().with_mermaid_runtime(true);
    let decide = |uri: &str| {
        enabled.resolve(&BrokerRequest {
            method: "GET",
            uri,
            controller_tab_id: 7,
            kind: ResourceKind::Script,
        })
    };
    let chars: Vec<char> = MERMAID_RUNTIME_URI.chars().collect();
    let mut variants = Vec::new();
    for index in 0..=chars.len() {
        for inserted in ['/', '.', '%', '?', '#', '\\', ' ', '\0', 'A', 'a', 'é'] {
            let mut edited = chars.clone();
            edited.insert(index, inserted);
            variants.push(edited.into_iter().collect::<String>());
        }
        if index < chars.len() {
            let mut deleted = chars.clone();
            deleted.remove(index);
            variants.push(deleted.into_iter().collect());
            let mut upper = chars.clone();
            upper[index] = upper[index].to_ascii_uppercase();
            variants.push(upper.into_iter().collect());
            let encoded = format!(
                "{}%{:02X}{}",
                chars[..index].iter().collect::<String>(),
                u32::from(chars[index]),
                chars[index + 1..].iter().collect::<String>()
            );
            variants.push(encoded);
        }
    }
    let mut checked = 0;
    for uri in variants.iter().filter(|uri| *uri != MERMAID_RUNTIME_URI) {
        assert_ne!(
            decide(uri),
            BrokerDecision::BundledRuntime(marknexia_webview::broker::BundledRuntime::Mermaid),
            "{uri:?}"
        );
        checked += 1;
    }
    assert!(checked > 500, "{checked}");
    assert_eq!(
        decide(MERMAID_RUNTIME_URI),
        BrokerDecision::BundledRuntime(marknexia_webview::broker::BundledRuntime::Mermaid)
    );
}
