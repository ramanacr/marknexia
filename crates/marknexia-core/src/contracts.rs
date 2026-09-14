use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub struct GenerationId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Heading {
    pub text: String,
    pub level: u8,
    pub slug: String,
    pub source_line: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source_line: Option<usize>,
}

impl Diagnostic {
    #[must_use]
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
            source_line: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum AppTheme {
    System,
    Light,
    Dark,
    HighContrast,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum NavigationIntent {
    OpenDocument {
        document_id: DocumentId,
        fragment: Option<BoundedFragment>,
    },
    OpenExternal {
        href: BoundedUrl,
    },
    Back,
    Forward,
    Reload,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct RenderRequest {
    pub document_id: DocumentId,
    pub generation: GenerationId,
    pub source: DocumentText,
    pub theme: AppTheme,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct RenderResult {
    pub generation: GenerationId,
    pub html: RenderedHtml,
    pub headings: Vec<Heading>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedValueError {
    kind: &'static str,
    max_bytes: usize,
    actual_bytes: usize,
}

impl fmt::Display for BoundedValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} is {} bytes, exceeding its {} byte limit",
            self.kind, self.actual_bytes, self.max_bytes
        )
    }
}

impl std::error::Error for BoundedValueError {}

macro_rules! bounded_string {
    ($name:ident, $kind:literal, $maximum:expr) => {
        #[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub const MAX_BYTES: usize = $maximum;

            pub fn try_new(value: impl Into<String>) -> Result<Self, BoundedValueError> {
                let value = value.into();
                let actual_bytes = value.len();
                if actual_bytes > Self::MAX_BYTES {
                    return Err(BoundedValueError {
                        kind: $kind,
                        max_bytes: Self::MAX_BYTES,
                        actual_bytes,
                    });
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

bounded_string!(DocumentId, "document id", 256);
bounded_string!(DocumentText, "document text", 4 * 1024 * 1024);
bounded_string!(RenderedHtml, "rendered HTML", 8 * 1024 * 1024);
bounded_string!(BoundedFragment, "fragment", 1024);
bounded_string!(BoundedUrl, "URL", 8 * 1024);
