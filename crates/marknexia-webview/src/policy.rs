//! Immutable content and bounded decisions shared by WebView2 callbacks.

use std::collections::BTreeMap;

use marknexia_security::{ContentPolicy, HtmlPolicy, SanitizedFragment};

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
struct Asset {
    kind: AssetType,
    bytes: AssetBytes,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AssetBytes {
    TrustedBundled(&'static [u8]),
    Generated(Vec<u8>),
}

impl Asset {
    fn as_bytes(&self) -> &[u8] {
        match &self.bytes {
            AssetBytes::TrustedBundled(bytes) => bytes,
            AssetBytes::Generated(bytes) => bytes,
        }
    }
}

/// An inert generated raster. Generated SVG, JavaScript and CSS have no public
/// construction path: `SanitizedSvg` is inline-only HTML-serialized markup,
/// not a standalone `image/svg+xml` document, so it is embedded through
/// `SanitizedSvg::into_fragment` rather than served as an asset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedAsset(Asset);

impl GeneratedAsset {
    pub fn raster(kind: AssetType, bytes: Vec<u8>) -> Result<Self, DocumentError> {
        if !matches!(kind, AssetType::Png | AssetType::Jpeg | AssetType::Gif) {
            return Err(DocumentError::UntrustedActiveAsset);
        }
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(DocumentError::TooLarge);
        }
        Ok(Self(Asset {
            kind,
            bytes: AssetBytes::Generated(bytes),
        }))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrustedBundledAsset(Asset);

impl TrustedBundledAsset {
    pub(crate) const fn new(kind: AssetType, bytes: &'static [u8]) -> Self {
        Self(Asset {
            kind,
            bytes: AssetBytes::TrustedBundled(bytes),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostDocument {
    tab_id: u64,
    document_epoch: u64,
    html: Vec<u8>,
    assets: BTreeMap<String, Asset>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentError {
    EmptyHtml,
    TooLarge,
    TooManyAssets,
    InvalidAssetPath,
    WrongAssetType,
    UntrustedActiveAsset,
}

impl HostDocument {
    pub fn new(
        tab_id: u64,
        document_epoch: u64,
        fragment: SanitizedFragment,
        assets: BTreeMap<String, GeneratedAsset>,
    ) -> Result<Self, DocumentError> {
        Self::from_shell(tab_id, document_epoch, None, fragment, assets)
    }

    /// Like [`HostDocument::new`], plus a document `<title>` that names the
    /// WebView document for assistive technology. `title` is plain text and
    /// is entity-encoded here, so callers cannot inject head markup.
    pub fn new_titled(
        tab_id: u64,
        document_epoch: u64,
        title: &str,
        fragment: SanitizedFragment,
        assets: BTreeMap<String, GeneratedAsset>,
    ) -> Result<Self, DocumentError> {
        let title = HtmlPolicy::new(ContentPolicy::default())
            .encode_text(title)
            .map_err(|_| DocumentError::TooLarge)?;
        Self::from_shell(tab_id, document_epoch, Some(title), fragment, assets)
    }

    fn from_shell(
        tab_id: u64,
        document_epoch: u64,
        title: Option<SanitizedFragment>,
        fragment: SanitizedFragment,
        assets: BTreeMap<String, GeneratedAsset>,
    ) -> Result<Self, DocumentError> {
        const HEAD: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"color-scheme\" content=\"light dark\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src 'self' data:; object-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'\">";
        const BODY: &str = "</head><body>";
        const SUFFIX: &str = "</body></html>";
        let title = title.as_ref().map_or("", SanitizedFragment::as_str);
        let title_markup = if title.is_empty() {
            0
        } else {
            "<title></title>".len()
        };
        let required = [
            HEAD.len(),
            title_markup,
            title.len(),
            BODY.len(),
            fragment.as_str().len(),
            SUFFIX.len(),
        ]
        .into_iter()
        .try_fold(0_usize, usize::checked_add)
        .ok_or(DocumentError::TooLarge)?;
        if required > MAX_DOCUMENT_BYTES {
            return Err(DocumentError::TooLarge);
        }
        let mut html = Vec::with_capacity(required);
        html.extend_from_slice(HEAD.as_bytes());
        if !title.is_empty() {
            html.extend_from_slice(b"<title>");
            html.extend_from_slice(title.as_bytes());
            html.extend_from_slice(b"</title>");
        }
        html.extend_from_slice(BODY.as_bytes());
        html.extend_from_slice(fragment.as_str().as_bytes());
        html.extend_from_slice(SUFFIX.as_bytes());
        Self::from_parts(
            tab_id,
            document_epoch,
            html,
            assets
                .into_iter()
                .map(|(path, asset)| (path, asset.0))
                .collect(),
        )
    }

    pub(crate) fn from_trusted_bundle(
        tab_id: u64,
        document_epoch: u64,
        html: Vec<u8>,
        assets: BTreeMap<String, TrustedBundledAsset>,
    ) -> Result<Self, DocumentError> {
        Self::from_parts(
            tab_id,
            document_epoch,
            html,
            assets
                .into_iter()
                .map(|(path, asset)| (path, asset.0))
                .collect(),
        )
    }

    fn from_parts(
        tab_id: u64,
        document_epoch: u64,
        html: Vec<u8>,
        assets: BTreeMap<String, Asset>,
    ) -> Result<Self, DocumentError> {
        let document = Self {
            tab_id,
            document_epoch,
            html,
            assets,
        };
        document.validate()?;
        Ok(document)
    }

    #[must_use]
    pub const fn tab_id(&self) -> u64 {
        self.tab_id
    }

    #[must_use]
    pub const fn document_epoch(&self) -> u64 {
        self.document_epoch
    }

    #[must_use]
    pub fn html(&self) -> &[u8] {
        &self.html
    }

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
            if asset.as_bytes().len() > MAX_ASSET_BYTES {
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
                .map(|asset| ResponseSpec::ok(asset.kind.content_type(), asset.as_bytes()))
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
