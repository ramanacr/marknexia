//! Immutable content and bounded decisions shared by WebView2 callbacks.

use std::{collections::BTreeMap, sync::Arc};

use marknexia_rendering::{PageIdentity, RenderLimits, RenderedDocument};
use marknexia_security::{ContentPolicy, HtmlPolicy, SanitizedFragment};

use crate::{
    broker::{BrokerDecision, BrokerRequest, ResourceBroker, TabResourceBroker},
    protocol::{MessageError, PageToHost, ProtocolContext, parse_page_message},
};

/// Limit for shell-built and bundled documents.
pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
/// Limit for a rendered Markdown page: the rendering crate's page limit plus
/// room for the host-inserted `<title>`.
pub const MAX_RENDERED_DOCUMENT_BYTES: usize =
    RenderLimits::DEFAULT_MAX_RENDERED_HTML_BYTES + MAX_TITLE_MARKUP_BYTES;
const MAX_TITLE_MARKUP_BYTES: usize = 64 * 1024;
/// Response-header policy for every broker response except a rendered page,
/// whose header repeats the page's own meta policy (see
/// [`HostDocument::from_rendered`]).
pub const DEFAULT_RESPONSE_CSP: &str = "default-src 'none'; img-src 'self'; style-src 'self'; script-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
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

/// An immutable document for one tab. Fields stay private: every constructor
/// builds the page from trusted text plus sealed parts, and nothing reopens
/// the bytes for mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostDocument {
    tab_id: u64,
    document_epoch: u64,
    /// Virtual origin that serves this document: the per-tab origin, or the
    /// rendered page's own identity origin.
    origin: String,
    /// Shared so that cloning a large rendered page stays cheap.
    html: Arc<[u8]>,
    max_html_bytes: usize,
    /// `Content-Security-Policy` response header for the document itself.
    document_csp: String,
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
    /// The rendered page was not produced for the supplied page identity, or
    /// its trusted template prefix is not the one this host understands.
    IdentityMismatch,
}

impl HostDocument {
    /// Hosts a page produced by `marknexia-rendering`: the .NET template with
    /// its inline bundled CSS and nonce'd bridge script around the sanitized
    /// body. `RenderedDocument` can only be built by the renderer, so the
    /// bytes are trusted template text plus a `SanitizedFragment`; the only
    /// host change is an entity-encoded `<title>` inserted into the template
    /// head. The document is served from `identity`'s origin so the page's
    /// `<base href>`, `base-uri` and `img-src` name the serving origin, and
    /// the document response carries the page's own policy as its header so
    /// the header and meta policies are identical. `identity` must be the one
    /// the page was rendered with; a mismatch is rejected.
    pub fn from_rendered(
        tab_id: u64,
        document_epoch: u64,
        title: &str,
        rendered: &RenderedDocument,
        identity: &PageIdentity,
    ) -> Result<Self, DocumentError> {
        const PREFIX: &str = "<!DOCTYPE html>\n<html lang=\"en\" data-theme=\"";
        const HEAD: &str = "\">\n<head>\n  <meta charset=\"utf-8\" />\n";
        const HEAD_END: &str = "\n  </style>\n</head>\n<body>\n";
        let page = rendered.page_html();
        let origin = identity.origin();
        let csp = rendered_page_csp(identity);
        // The theme value is one of three fixed template words, so the head
        // opening is at a bounded offset in trusted template text.
        let theme_end = page
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.get(..16))
            .and_then(|window| window.find('"'))
            .map(|offset| PREFIX.len() + offset)
            .ok_or(DocumentError::IdentityMismatch)?;
        if !page[theme_end..].starts_with(HEAD) {
            return Err(DocumentError::IdentityMismatch);
        }
        let insert_at = theme_end + HEAD.len();
        // The first `</style></head><body>` ends the template head: the
        // bundled stylesheet precedes it and contains no such text.
        let head_end = page.find(HEAD_END).ok_or(DocumentError::IdentityMismatch)?;
        let head = &page[..head_end];
        let expected_meta = format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\" />",
            html_encode(&csp)
        );
        let expected_base = format!("<base href=\"{}/", html_encode(&origin));
        if !head.contains(&expected_meta) || !head.contains(&expected_base) {
            return Err(DocumentError::IdentityMismatch);
        }
        let title = HtmlPolicy::new(ContentPolicy::default())
            .encode_text(title)
            .map_err(|_| DocumentError::TooLarge)?;
        let title_markup = format!("  <title>{}</title>\n", title.as_str());
        if title_markup.len() > MAX_TITLE_MARKUP_BYTES {
            return Err(DocumentError::TooLarge);
        }
        let required = page
            .len()
            .checked_add(title_markup.len())
            .ok_or(DocumentError::TooLarge)?;
        if required > MAX_RENDERED_DOCUMENT_BYTES {
            return Err(DocumentError::TooLarge);
        }
        let mut html = Vec::with_capacity(required);
        html.extend_from_slice(&page.as_bytes()[..insert_at]);
        html.extend_from_slice(title_markup.as_bytes());
        html.extend_from_slice(&page.as_bytes()[insert_at..]);
        drop(page);
        let document = Self {
            tab_id,
            document_epoch,
            origin,
            html: html.into(),
            max_html_bytes: MAX_RENDERED_DOCUMENT_BYTES,
            document_csp: csp,
            assets: BTreeMap::new(),
        };
        document.validate()?;
        Ok(document)
    }

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
            origin: TabResourceBroker::for_tab(tab_id).origin().to_owned(),
            html: html.into(),
            max_html_bytes: MAX_DOCUMENT_BYTES,
            document_csp: DEFAULT_RESPONSE_CSP.to_owned(),
            assets,
        };
        document.validate()?;
        Ok(document)
    }

    fn broker(&self) -> TabResourceBroker {
        TabResourceBroker::with_origin(self.tab_id, self.origin.clone())
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
        if self.html.len() > self.max_html_bytes {
            return Err(DocumentError::TooLarge);
        }
        if self.assets.len() > MAX_ASSETS {
            return Err(DocumentError::TooManyAssets);
        }
        let broker = self.broker();
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
        self.broker().document_uri()
    }

    /// The `Content-Security-Policy` header sent with the document response.
    #[must_use]
    pub fn document_csp(&self) -> &str {
        &self.document_csp
    }

    pub fn resolve(&self, request: &BrokerRequest<'_>) -> ResponseSpec<'_> {
        match self.broker().resolve(request) {
            BrokerDecision::Document => ResponseSpec {
                content_security_policy: &self.document_csp,
                ..ResponseSpec::ok("text/html; charset=utf-8", &self.html)
            },
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
        let broker = self.broker();
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
    pub content_security_policy: &'a str,
    pub body: &'a [u8],
}

impl<'a> ResponseSpec<'a> {
    pub const fn ok(content_type: &'static str, body: &'a [u8]) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type,
            content_security_policy: DEFAULT_RESPONSE_CSP,
            body,
        }
    }

    pub const fn forbidden() -> Self {
        Self {
            status: 403,
            reason: "Forbidden",
            content_type: "text/plain; charset=utf-8",
            content_security_policy: DEFAULT_RESPONSE_CSP,
            body: b"Forbidden",
        }
    }

    pub const fn method_not_allowed() -> Self {
        Self {
            status: 405,
            reason: "Method Not Allowed",
            content_type: "text/plain; charset=utf-8",
            content_security_policy: DEFAULT_RESPONSE_CSP,
            body: b"Method Not Allowed",
        }
    }
}

/// The policy `marknexia-rendering` writes into the page's meta tag for
/// `identity` with remote assets off (.NET `TemplateEngine`). It names the
/// identity origin and nonce; `from_rendered` checks the page against it, so
/// any template change fails closed instead of drifting.
fn rendered_page_csp(identity: &PageIdentity) -> String {
    let origin = identity.origin();
    let nonce = identity.nonce();
    format!(
        "default-src 'none'; base-uri {origin}; script-src 'nonce-{nonce}' https://marknexia.assets; style-src 'unsafe-inline'; img-src {origin} data:; font-src 'none'; object-src 'none'; frame-src 'none'; connect-src 'none'; form-action 'none'"
    )
}

/// .NET `WebUtility.HtmlEncode` for the five characters the template encodes.
fn html_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for character in text.chars() {
        match character {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}
