//! Portable document opening: startup paths, bounded reads, .NET-compatible
//! decoding, off-thread rendering, and stale-result discard. No HWND or COM.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt, fs,
    path::{Path, PathBuf},
};

use marknexia_core::contracts::AppTheme;
use marknexia_files::bounded_read::{BoundedReadError, BoundedReader};
use marknexia_rendering::{
    DocumentRenderer, PageIdentity, RenderContext, RenderError, RenderedDocument, Renderer,
};

/// Paths to open from the command line (arguments after the program name).
/// Every non-empty argument is a path; one leading `--` separator is
/// dropped. Relative paths resolve against the current directory.
#[must_use]
pub fn startup_paths<I>(arguments: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = OsString>,
{
    let mut separator_seen = false;
    arguments
        .into_iter()
        .filter(|argument| {
            if !separator_seen && argument == "--" {
                separator_seen = true;
                return false;
            }
            !argument.is_empty()
        })
        .map(|argument| {
            let path = PathBuf::from(argument);
            std::path::absolute(&path).unwrap_or(path)
        })
        .collect()
}

/// Tab title for a document: its file name.
#[must_use]
pub fn tab_title(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The encoding `decode_source` detected (.NET `FileService.DecodeBytes`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Utf32Be,
    Latin1,
}

/// .NET `FileService.DecodeBytes`, including its quirks:
/// - a UTF-8 BOM is stripped and the rest decoded with replacement;
/// - `FF FE` always means UTF-16 LE (the UTF-32 LE branch is unreachable
///   in .NET because the UTF-16 LE check runs first);
/// - `00 00 FE FF` is labelled UTF-32 BE but decoded with little-endian
///   `Encoding.UTF32`, exactly as .NET does;
/// - otherwise strict UTF-8, falling back to Latin-1.
///
/// Invalid UTF-16/UTF-32 code units become U+FFFD, like .NET's decoders.
#[must_use]
pub fn decode_source(bytes: &[u8]) -> (String, SourceEncoding) {
    if bytes.is_empty() {
        return (String::new(), SourceEncoding::Utf8);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (
            String::from_utf8_lossy(rest).into_owned(),
            SourceEncoding::Utf8Bom,
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (
            decode_utf16(rest, u16::from_le_bytes),
            SourceEncoding::Utf16Le,
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (
            decode_utf16(rest, u16::from_be_bytes),
            SourceEncoding::Utf16Be,
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0x00, 0x00, 0xFE, 0xFF]) {
        return (decode_utf32_le(rest), SourceEncoding::Utf32Be);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), SourceEncoding::Utf8),
        Err(_) => (
            bytes.iter().copied().map(char::from).collect(),
            SourceEncoding::Latin1,
        ),
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let (pairs, rest) = bytes.as_chunks::<2>();
    let mut text: String = char::decode_utf16(pairs.iter().map(|pair| unit(*pair)))
        .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    if !rest.is_empty() {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    text
}

fn decode_utf32_le(bytes: &[u8]) -> String {
    let (quads, rest) = bytes.as_chunks::<4>();
    let mut text: String = quads
        .iter()
        .map(|quad| {
            char::from_u32(u32::from_le_bytes(*quad)).unwrap_or(char::REPLACEMENT_CHARACTER)
        })
        .collect();
    if !rest.is_empty() {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    text
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenError {
    NotFound,
    Io(String),
    Render(RenderError),
    Identity(String),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the file does not exist"),
            Self::Io(error) => write!(f, "the file could not be read ({error})"),
            Self::Render(error) => error.fmt(f),
            Self::Identity(error) => write!(f, "no page identity ({error})"),
        }
    }
}

/// Reads a Markdown source like .NET `FileService.ReadTextAsync`: the size is
/// checked with `check_size` (the renderer's `check_source_size`) before the
/// file is read, the read itself is bounded by `limit` (the file may grow
/// after the check), and the bytes are decoded by [`decode_source`].
pub fn read_source(
    path: &Path,
    check_size: impl Fn(u64) -> Result<(), RenderError>,
    limit: u64,
) -> Result<String, OpenError> {
    let file = fs::File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => OpenError::NotFound,
        _ => OpenError::Io(error.kind().to_string()),
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| OpenError::Io(error.kind().to_string()))?;
    if !metadata.is_file() {
        return Err(OpenError::Io("not a file".to_owned()));
    }
    check_size(metadata.len()).map_err(OpenError::Render)?;
    let bytes = BoundedReader::new(usize::try_from(limit).unwrap_or(usize::MAX))
        .read_from(file)
        .map_err(|error| match error {
            BoundedReadError::TooLarge => OpenError::Render(RenderError::SourceTooLarge {
                size_bytes: limit.saturating_add(1),
                maximum_bytes: limit,
            }),
            BoundedReadError::Io => OpenError::Io("read failed".to_owned()),
        })?;
    Ok(decode_source(&bytes).0)
}

/// Reads and renders one file. Runs on a worker thread: everything it
/// touches is `Send` and owned.
pub fn render_file(
    path: &Path,
    identity: PageIdentity,
    theme: AppTheme,
) -> Result<RenderedDocument, OpenError> {
    let renderer = Renderer::new();
    let source = read_source(
        path,
        |size| renderer.check_source_size(size),
        renderer.limits().max_source_bytes,
    )?;
    renderer
        .render(&source, &RenderContext::new(theme, identity))
        .map_err(OpenError::Render)
}

/// Pending renders by tab. The renderer cannot be cancelled, so a result is
/// accepted only while its ticket is still the tab's current one: closing the
/// tab (or starting a newer render for it) makes an in-flight result stale.
#[derive(Debug, Default)]
pub struct RenderTickets {
    next: u64,
    pending: BTreeMap<u64, u64>,
}

impl RenderTickets {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a render for `tab_id`, superseding any earlier one.
    pub fn begin(&mut self, tab_id: u64) -> u64 {
        self.next = self.next.wrapping_add(1);
        self.pending.insert(tab_id, self.next);
        self.next
    }

    /// The tab closed; any in-flight result for it will be discarded.
    pub fn cancel(&mut self, tab_id: u64) {
        self.pending.remove(&tab_id);
    }

    /// True exactly once for the current ticket of a live tab.
    pub fn complete(&mut self, tab_id: u64, ticket: u64) -> bool {
        if self.pending.get(&tab_id) == Some(&ticket) {
            self.pending.remove(&tab_id);
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn is_pending(&self, tab_id: u64) -> bool {
        self.pending.contains_key(&tab_id)
    }
}

/// A worker's result, posted back to the UI thread.
pub struct RenderOutcome {
    pub tab_id: u64,
    pub ticket: u64,
    pub result: Result<(RenderedDocument, PageIdentity), OpenError>,
}

const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<RenderOutcome>();
};
