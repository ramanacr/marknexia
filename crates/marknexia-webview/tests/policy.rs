use std::collections::BTreeMap;

use marknexia_webview::{
    broker::{BrokerRequest, ResourceKind},
    policy::{Asset, AssetType, DocumentError, HostDocument, MAX_ASSET_BYTES},
    protocol::{MessageError, PageToHost},
};

fn document() -> HostDocument {
    HostDocument {
        tab_id: 7,
        document_epoch: 3,
        html: b"<p>probe</p>".to_vec(),
        assets: BTreeMap::from([(
            "probe.css".to_owned(),
            Asset {
                kind: AssetType::Css,
                bytes: b"body{}".to_vec(),
            },
        )]),
    }
}

#[test]
fn immutable_document_serves_only_its_own_bounded_resources() {
    let document = document();
    document.validate().unwrap();
    let request = BrokerRequest {
        method: "GET",
        uri: "https://tab-7.marknexia.invalid/document",
        controller_tab_id: 7,
        kind: ResourceKind::Document,
    };
    assert_eq!(document.resolve(&request).body, b"<p>probe</p>");
    let denied = BrokerRequest {
        controller_tab_id: 8,
        ..request
    };
    assert_eq!(document.resolve(&denied).status, 403);
}

#[test]
fn oversized_or_misnamed_assets_never_enter_the_host() {
    let mut document = document();
    document.assets.get_mut("probe.css").unwrap().bytes = vec![0; MAX_ASSET_BYTES + 1];
    assert_eq!(document.validate(), Err(DocumentError::TooLarge));
    document.assets.get_mut("probe.css").unwrap().bytes.clear();
    document.assets.insert(
        "probe.js".to_owned(),
        Asset {
            kind: AssetType::Png,
            bytes: vec![1],
        },
    );
    assert_eq!(document.validate(), Err(DocumentError::WrongAssetType));
}

#[test]
fn navigation_and_message_identity_are_bound_to_one_document() {
    let document = document();
    assert!(document.permits_navigation("https://tab-7.marknexia.invalid/document"));
    assert!(!document.permits_navigation("about:blank"));
    assert!(!document.permits_navigation("https://example.com/"));
    let json = br#"{"type":"ready","payload":{"protocol":1,"tabId":7,"documentEpoch":3}}"#;
    assert!(matches!(
        document.parse_message(json, &document.document_uri()),
        Ok(PageToHost::Ready { .. })
    ));
    assert_eq!(
        document.parse_message(json, "https://tab-8.marknexia.invalid/document"),
        Err(MessageError::WrongOrigin)
    );
}
