//! Strict page-to-host messages for a single WebView document and tab.

use std::io::{self, Write};

use marknexia_core::contracts::AppTheme;
use marknexia_core::contracts::BoundedUrl;
use marknexia_security::SanitizedFragment;
use serde::{Deserialize, Serialize};

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_HOST_MESSAGE_BYTES: usize = 8 * 1024 * 1024 + 64 * 1024;
/// Longest find query, in UTF-16 code units (JavaScript string length), in
/// either direction. The page adapter enforces the same limit.
pub const MAX_FIND_QUERY_UNITS: usize = 1024;
/// Largest match count a page may report for one query.
pub const MAX_FIND_MATCHES: u32 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HostToPage {
    RenderDocument {
        protocol: u16,
        tab_id: u64,
        document_epoch: u64,
        html: SanitizedFragment,
    },
    SetTheme {
        protocol: u16,
        tab_id: u64,
        document_epoch: u64,
        theme: AppTheme,
    },
    /// Runs the page bridge's `findText(query, backwards)`. The page answers
    /// with [`PageToHost::FindResult`] or [`PageToHost::FindUnavailable`]
    /// that carries the same `request_id`.
    Find {
        protocol: u16,
        tab_id: u64,
        document_epoch: u64,
        request_id: u64,
        query: String,
        backwards: bool,
    },
    /// Runs the page bridge's `clearSearch()`. The page does not answer.
    ClearFind {
        protocol: u16,
        tab_id: u64,
        document_epoch: u64,
    },
}

impl HostToPage {
    pub const fn identity(&self) -> (u16, u64, u64) {
        match self {
            Self::RenderDocument {
                protocol,
                tab_id,
                document_epoch,
                ..
            }
            | Self::SetTheme {
                protocol,
                tab_id,
                document_epoch,
                ..
            }
            | Self::Find {
                protocol,
                tab_id,
                document_epoch,
                ..
            }
            | Self::ClearFind {
                protocol,
                tab_id,
                document_epoch,
            } => (*protocol, *tab_id, *document_epoch),
        }
    }

    /// Content limits that the identity check does not cover.
    fn validate(&self) -> Result<(), MessageError> {
        match self {
            Self::Find {
                request_id, query, ..
            } => {
                if *request_id == 0 {
                    Err(MessageError::InvalidPayload)
                } else if utf16_len(query) > MAX_FIND_QUERY_UNITS {
                    Err(MessageError::TooLarge)
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }
}

/// JavaScript string length of `text`.
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

pub fn serialize_host_message(message: &HostToPage) -> Result<String, MessageError> {
    message.validate()?;
    let mut writer = CappedWriter::new(MAX_HOST_MESSAGE_BYTES);
    if serde_json::to_writer(&mut writer, message).is_err() {
        return Err(if writer.exceeded {
            MessageError::TooLarge
        } else {
            MessageError::InvalidPayload
        });
    }
    String::from_utf8(writer.bytes).map_err(|_| MessageError::InvalidPayload)
}

struct CappedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl CappedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(4096)),
            limit,
            exceeded: false,
        }
    }
}

impl Write for CappedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if buffer.len() > remaining {
            self.exceeded = true;
            return Err(io::Error::other("host message exceeds byte cap"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod capped_writer_tests {
    use std::io::Write;

    use super::CappedWriter;

    #[test]
    fn rejects_before_extending_past_cap() {
        let mut writer = CappedWriter::new(4);
        writer.write_all(b"1234").unwrap();
        assert!(writer.write_all(b"5").is_err());
        assert!(writer.exceeded);
        assert_eq!(writer.bytes, b"1234");
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PageToHost {
    Ready {
        protocol: u16,
        document_epoch: u64,
        tab_id: u64,
    },
    OpenLink {
        protocol: u16,
        document_epoch: u64,
        tab_id: u64,
        href: String,
    },
    CopyText {
        protocol: u16,
        document_epoch: u64,
        tab_id: u64,
        text: String,
    },
    FocusChanged {
        protocol: u16,
        document_epoch: u64,
        tab_id: u64,
        focused: bool,
    },
    /// The page bridge's `findText` result for `request_id`. `current_match`
    /// is 1-based, and 0 when there is no match.
    FindResult {
        protocol: u16,
        document_epoch: u64,
        tab_id: u64,
        request_id: u64,
        query: String,
        match_count: u32,
        current_match: u32,
    },
    /// The page bridge could not run `findText` for `request_id`: it is not
    /// loaded yet, or it failed. .NET shows "Document loading…" for this.
    FindUnavailable {
        protocol: u16,
        document_epoch: u64,
        tab_id: u64,
        request_id: u64,
    },
}

impl PageToHost {
    fn identity(&self) -> (u16, u64, u64) {
        match self {
            Self::Ready {
                protocol,
                document_epoch,
                tab_id,
            }
            | Self::OpenLink {
                protocol,
                document_epoch,
                tab_id,
                ..
            }
            | Self::CopyText {
                protocol,
                document_epoch,
                tab_id,
                ..
            }
            | Self::FocusChanged {
                protocol,
                document_epoch,
                tab_id,
                ..
            }
            | Self::FindResult {
                protocol,
                document_epoch,
                tab_id,
                ..
            }
            | Self::FindUnavailable {
                protocol,
                document_epoch,
                tab_id,
                ..
            } => (*protocol, *tab_id, *document_epoch),
        }
    }

    /// Payload rules beyond the JSON shape. A find result must describe a
    /// consistent position (the `DocumentSearchState` rules) within bounds.
    fn validate_payload(&self) -> Result<(), MessageError> {
        let valid = match self {
            Self::OpenLink { href, .. } => BoundedUrl::try_new(href.clone()).is_ok(),
            Self::FindResult {
                request_id,
                query,
                match_count,
                current_match,
                ..
            } => {
                *request_id > 0
                    && utf16_len(query) <= MAX_FIND_QUERY_UNITS
                    && *match_count <= MAX_FIND_MATCHES
                    && current_match <= match_count
                    && (*match_count == 0 || *current_match > 0)
                    && (!query.is_empty() || *match_count == 0)
            }
            Self::FindUnavailable { request_id, .. } => *request_id > 0,
            Self::Ready { .. } | Self::CopyText { .. } | Self::FocusChanged { .. } => true,
        };
        if valid {
            Ok(())
        } else {
            Err(MessageError::InvalidPayload)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolContext<'a> {
    pub expected_source_uri: &'a str,
    pub expected_origin: &'a str,
    pub protocol: u16,
    pub tab_id: u64,
    pub document_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageError {
    TooLarge,
    WrongOrigin,
    WrongSource,
    WrongProtocol,
    CrossTab,
    StaleEpoch,
    InvalidPayload,
}

/// The native adapter supplies `source_uri` from WebView2's event args, never
/// from the untrusted JSON body. Controller identity is bound by the context.
pub fn parse_page_message(
    raw_json: &[u8],
    source_uri: &str,
    context: &ProtocolContext<'_>,
) -> Result<PageToHost, MessageError> {
    if raw_json.len() > MAX_MESSAGE_BYTES {
        return Err(MessageError::TooLarge);
    }

    if !source_uri
        .strip_prefix(context.expected_origin)
        .is_some_and(|suffix| suffix.starts_with('/'))
    {
        return Err(MessageError::WrongOrigin);
    }
    if source_uri != context.expected_source_uri {
        return Err(MessageError::WrongSource);
    }

    let message: PageToHost =
        serde_json::from_slice(raw_json).map_err(|_| MessageError::InvalidPayload)?;
    message.validate_payload()?;
    let (protocol, tab_id, document_epoch) = message.identity();
    if protocol != context.protocol {
        return Err(MessageError::WrongProtocol);
    }
    if tab_id != context.tab_id {
        return Err(MessageError::CrossTab);
    }
    if document_epoch != context.document_epoch {
        return Err(MessageError::StaleEpoch);
    }
    Ok(message)
}
