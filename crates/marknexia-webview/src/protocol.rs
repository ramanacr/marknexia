//! Strict page-to-host messages for a single WebView document and tab.

use std::io::{self, Write};

use marknexia_core::contracts::AppTheme;
use marknexia_core::contracts::BoundedUrl;
use marknexia_security::SanitizedFragment;
use serde::{Deserialize, Serialize};

pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_HOST_MESSAGE_BYTES: usize = 8 * 1024 * 1024 + 64 * 1024;

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
            } => (*protocol, *tab_id, *document_epoch),
        }
    }
}

pub fn serialize_host_message(message: &HostToPage) -> Result<String, MessageError> {
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
            } => (*protocol, *tab_id, *document_epoch),
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
    if let PageToHost::OpenLink { href, .. } = &message {
        BoundedUrl::try_new(href.clone()).map_err(|_| MessageError::InvalidPayload)?;
    }
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
