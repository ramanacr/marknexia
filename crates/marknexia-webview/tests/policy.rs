use std::collections::BTreeMap;

use marknexia_security::{ContentPolicy, HtmlPolicy};
use marknexia_webview::{
    broker::{BrokerRequest, ResourceKind},
    policy::{AssetType, DocumentError, GeneratedAsset, HostDocument, MAX_ASSET_BYTES},
    protocol::{MessageError, PageToHost},
};

fn document() -> HostDocument {
    let fragment = HtmlPolicy::new(ContentPolicy::default())
        .encode_text("probe")
        .unwrap();
    HostDocument::new(
        7,
        3,
        fragment,
        BTreeMap::from([(
            "probe.png".to_owned(),
            GeneratedAsset::raster(AssetType::Png, vec![1, 2, 3]).unwrap(),
        )]),
    )
    .unwrap()
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
    assert!(String::from_utf8_lossy(document.resolve(&request).body).contains("probe"));
    let denied = BrokerRequest {
        controller_tab_id: 8,
        ..request
    };
    assert_eq!(document.resolve(&denied).status, 403);
}

#[test]
fn oversized_or_misnamed_assets_never_enter_the_host() {
    assert_eq!(
        GeneratedAsset::raster(AssetType::Png, vec![0; MAX_ASSET_BYTES + 1]),
        Err(DocumentError::TooLarge)
    );
    let fragment = HtmlPolicy::new(ContentPolicy::default())
        .encode_text("x")
        .unwrap();
    let result = HostDocument::new(
        7,
        3,
        fragment,
        BTreeMap::from([(
            "probe.js".to_owned(),
            GeneratedAsset::raster(AssetType::Png, vec![1]).unwrap(),
        )]),
    );
    assert_eq!(result, Err(DocumentError::WrongAssetType));
    assert_eq!(
        GeneratedAsset::raster(AssetType::JavaScript, b"alert(1)".to_vec()),
        Err(DocumentError::UntrustedActiveAsset)
    );
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
