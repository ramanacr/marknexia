use marknexia_webview::{
    broker::{BrokerRequest, ResourceKind},
    probe,
};

#[test]
fn probe_snapshots_have_distinct_origins_and_valid_bundled_assets() {
    let first = probe::document(7, 1);
    let second = probe::document(8, 1);
    first.validate().unwrap();
    second.validate().unwrap();
    assert_ne!(first.document_uri(), second.document_uri());
    assert!(String::from_utf8_lossy(&first.html).contains("Tab 7"));
    let request = BrokerRequest {
        method: "GET",
        uri: "https://tab-7.marknexia.invalid/assets/probe-image.svg",
        controller_tab_id: 7,
        kind: ResourceKind::Image,
    };
    assert_eq!(first.resolve(&request).status, 200);
    assert_eq!(second.resolve(&request).status, 403);
}
