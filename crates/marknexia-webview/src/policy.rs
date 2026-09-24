//! Immutable content and bounded decisions shared by WebView2 callbacks.

use std::collections::BTreeMap;

use crate::{
    broker::{BrokerDecision, BrokerRequest, ResourceBroker, TabResourceBroker},
    protocol::{MessageError, PageToHost, ProtocolContext, parse_page_message},
};

pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ASSET_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ASSETS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetType {
    Css,
    JavaScript,
    Png,
    Jpeg,
    Gif,
    Svg,
}

impl AssetType {
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Css => "text/css; charset=utf-8",
            Self::JavaScript => "text/javascript; charset=utf-8",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Svg => "image/svg+xml",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub kind: AssetType,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostDocument {
    pub tab_id: u64,
    pub document_epoch: u64,
    pub html: Vec<u8>,
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentError {
    EmptyHtml,
    TooLarge,
    TooManyAssets,
    InvalidAssetPath,
    WrongAssetType,
}

impl HostDocument {
    pub fn validate(&self) -> Result<(), DocumentError> {
        if self.html.is_empty() {
            return Err(DocumentError::EmptyHtml);
        }
        if self.html.len() > MAX_DOCUMENT_BYTES {
            return Err(DocumentError::TooLarge);
        }
        if self.assets.len() > MAX_ASSETS {
            return Err(DocumentError::TooManyAssets);
        }
        let broker = TabResourceBroker::for_tab(self.tab_id);
        for (path, asset) in &self.assets {
            if asset.bytes.len() > MAX_ASSET_BYTES {
                return Err(DocumentError::TooLarge);
            }
            let kind = match asset.kind {
                AssetType::Css => crate::broker::ResourceKind::Stylesheet,
                AssetType::JavaScript => crate::broker::ResourceKind::Script,
                AssetType::Png | AssetType::Jpeg | AssetType::Gif | AssetType::Svg => {
                    crate::broker::ResourceKind::Image
                }
            };
            let uri = format!("{}/assets/{path}", broker.origin());
            if !matches!(
                broker.resolve(&BrokerRequest {
                    method: "GET",
                    uri: &uri,
                    controller_tab_id: self.tab_id,
                    kind,
                }),
                BrokerDecision::LocalAsset { relative_path } if relative_path == *path
            ) {
                return Err(DocumentError::InvalidAssetPath);
            }
            let extension_matches = match asset.kind {
                AssetType::Css => path.ends_with(".css"),
                AssetType::JavaScript => path.ends_with(".js"),
                AssetType::Png => path.ends_with(".png"),
                AssetType::Jpeg => path.ends_with(".jpg") || path.ends_with(".jpeg"),
                AssetType::Gif => path.ends_with(".gif"),
                AssetType::Svg => path.ends_with(".svg"),
            };
            if !extension_matches {
                return Err(DocumentError::WrongAssetType);
            }
        }
        Ok(())
    }

    pub fn document_uri(&self) -> String {
        TabResourceBroker::for_tab(self.tab_id).document_uri()
    }

    pub fn resolve(&self, request: &BrokerRequest<'_>) -> ResponseSpec<'_> {
        match TabResourceBroker::for_tab(self.tab_id).resolve(request) {
            BrokerDecision::Document => ResponseSpec::ok("text/html; charset=utf-8", &self.html),
            BrokerDecision::LocalAsset { relative_path } => self
                .assets
                .get(&relative_path)
                .map(|asset| ResponseSpec::ok(asset.kind.content_type(), &asset.bytes))
                .unwrap_or_else(ResponseSpec::forbidden),
            BrokerDecision::Deny { status: 405 } => ResponseSpec::method_not_allowed(),
            BrokerDecision::Deny { .. } => ResponseSpec::forbidden(),
        }
    }

    pub fn permits_navigation(&self, uri: &str) -> bool {
        uri == self.document_uri()
    }

    pub fn parse_message(
        &self,
        raw_json: &[u8],
        source_uri: &str,
    ) -> Result<PageToHost, MessageError> {
        let broker = TabResourceBroker::for_tab(self.tab_id);
        let source = broker.document_uri();
        parse_page_message(
            raw_json,
            source_uri,
            &ProtocolContext {
                expected_source_uri: &source,
                expected_origin: broker.origin(),
                protocol: 1,
                tab_id: self.tab_id,
                document_epoch: self.document_epoch,
            },
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseSpec<'a> {
    pub status: i32,
    pub reason: &'static str,
    pub content_type: &'static str,
    pub body: &'a [u8],
}

impl<'a> ResponseSpec<'a> {
    pub const fn ok(content_type: &'static str, body: &'a [u8]) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type,
            body,
        }
    }

    pub const fn forbidden() -> Self {
        Self {
            status: 403,
            reason: "Forbidden",
            content_type: "text/plain; charset=utf-8",
            body: b"Forbidden",
        }
    }

    pub const fn method_not_allowed() -> Self {
        Self {
            status: 405,
            reason: "Method Not Allowed",
            content_type: "text/plain; charset=utf-8",
            body: b"Method Not Allowed",
        }
    }
}
