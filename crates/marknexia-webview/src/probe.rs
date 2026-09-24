//! Inert, text-only two-tab native probe fixture.

use std::collections::BTreeMap;

use crate::policy::{Asset, AssetType, HostDocument};

#[must_use]
pub fn document(tab_id: u64, document_epoch: u64) -> HostDocument {
    let html = include_str!("../assets/probe.html")
        .replace("{{TAB_ID}}", &tab_id.to_string())
        .replace("{{DOCUMENT_EPOCH}}", &document_epoch.to_string())
        .into_bytes();
    let assets = BTreeMap::from([
        (
            "probe.css".to_owned(),
            Asset {
                kind: AssetType::Css,
                bytes: include_bytes!("../assets/probe.css").to_vec(),
            },
        ),
        (
            "probe.js".to_owned(),
            Asset {
                kind: AssetType::JavaScript,
                bytes: include_bytes!("../assets/probe.js").to_vec(),
            },
        ),
        (
            "probe-image.svg".to_owned(),
            Asset {
                kind: AssetType::Svg,
                bytes: include_bytes!("../assets/probe-image.svg").to_vec(),
            },
        ),
    ]);
    HostDocument {
        tab_id,
        document_epoch,
        html,
        assets,
    }
}
