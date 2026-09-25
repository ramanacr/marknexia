//! Inert, text-only two-tab native probe fixture.

use std::collections::BTreeMap;

use crate::policy::{AssetType, HostDocument, TrustedBundledAsset};

#[must_use]
pub fn document(tab_id: u64, document_epoch: u64) -> HostDocument {
    let html = include_str!("../assets/probe.html")
        .replace("{{TAB_ID}}", &tab_id.to_string())
        .replace("{{DOCUMENT_EPOCH}}", &document_epoch.to_string())
        .into_bytes();
    let assets = BTreeMap::from([
        (
            "probe.css".to_owned(),
            TrustedBundledAsset::new(AssetType::Css, include_bytes!("../assets/probe.css")),
        ),
        (
            "probe.js".to_owned(),
            TrustedBundledAsset::new(AssetType::JavaScript, include_bytes!("../assets/probe.js")),
        ),
        (
            "probe-image.svg".to_owned(),
            TrustedBundledAsset::new(AssetType::Svg, include_bytes!("../assets/probe-image.svg")),
        ),
    ]);
    HostDocument::from_trusted_bundle(tab_id, document_epoch, html, assets)
        .expect("compile-time probe bundle must satisfy document policy")
}
