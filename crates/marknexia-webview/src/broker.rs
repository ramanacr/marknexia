//! Fail-closed virtual-origin routing before any local file handle is opened.

use percent_encoding::percent_decode_str;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Document,
    Image,
    Script,
    Stylesheet,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrokerRequest<'a> {
    pub method: &'a str,
    pub uri: &'a str,
    /// Supplied by the controller event registration, not the requested URI.
    pub controller_tab_id: u64,
    pub kind: ResourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerDecision {
    Document,
    LocalAsset { relative_path: String },
    Deny { status: u16 },
}

pub trait ResourceBroker {
    fn resolve(&self, request: &BrokerRequest<'_>) -> BrokerDecision;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabResourceBroker {
    tab_id: u64,
    origin: String,
}

impl TabResourceBroker {
    #[must_use]
    pub fn for_tab(tab_id: u64) -> Self {
        Self {
            tab_id,
            origin: format!("https://tab-{tab_id}.marknexia.invalid"),
        }
    }

    /// A broker for a document served from its own virtual origin (a rendered
    /// page's identity origin). `origin` is trusted host-generated text.
    pub(crate) fn with_origin(tab_id: u64, origin: String) -> Self {
        Self { tab_id, origin }
    }

    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    #[must_use]
    pub fn document_uri(&self) -> String {
        format!("{}/document", self.origin)
    }
}

impl ResourceBroker for TabResourceBroker {
    fn resolve(&self, request: &BrokerRequest<'_>) -> BrokerDecision {
        if !request.method.eq_ignore_ascii_case("GET") {
            return BrokerDecision::Deny { status: 405 };
        }
        if request.controller_tab_id != self.tab_id {
            return BrokerDecision::Deny { status: 403 };
        }
        if request.kind == ResourceKind::Document && request.uri == self.document_uri() {
            return BrokerDecision::Document;
        }
        let Some(raw_path) = request
            .uri
            .strip_prefix(&self.origin)
            .and_then(|suffix| suffix.strip_prefix("/assets/"))
        else {
            return BrokerDecision::Deny { status: 403 };
        };
        let Some(relative_path) = decode_safe_relative_path(raw_path) else {
            return BrokerDecision::Deny { status: 403 };
        };
        let allowed = match request.kind {
            ResourceKind::Image => true,
            ResourceKind::Stylesheet => relative_path == "probe.css",
            ResourceKind::Script => relative_path == "probe.js",
            ResourceKind::Document | ResourceKind::Other => false,
        };
        if !allowed {
            return BrokerDecision::Deny { status: 403 };
        }
        BrokerDecision::LocalAsset { relative_path }
    }
}

fn decode_safe_relative_path(raw_path: &str) -> Option<String> {
    if raw_path.is_empty() || raw_path.len() > 8 * 1024 {
        return None;
    }
    let mut segments = Vec::new();
    for raw_segment in raw_path.split('/') {
        let decoded = percent_decode_str(raw_segment).decode_utf8().ok()?;
        if decoded.is_empty()
            || decoded == "."
            || decoded == ".."
            || decoded.ends_with('.')
            || decoded.ends_with(' ')
            || is_reserved_windows_device_name(&decoded)
            || decoded.chars().any(|character| {
                character.is_control()
                    || matches!(
                        character,
                        '/' | '\\' | ':' | '%' | '?' | '#' | '<' | '>' | '"' | '|' | '*'
                    )
            })
        {
            return None;
        }
        segments.push(decoded.into_owned());
    }
    Some(segments.join("/"))
}

fn is_reserved_windows_device_name(segment: &str) -> bool {
    // Win32 resolves these names as devices even beneath a directory and even
    // when an extension follows. Check every path component before opening it.
    let stem = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    for prefix in ["COM", "LPT"] {
        if let Some(suffix) = stem.strip_prefix(prefix) {
            return matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            );
        }
    }
    false
}
