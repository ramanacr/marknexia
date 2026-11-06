//! A strict, bounded reader for the portable ZIP.
//!
//! Only what a release ZIP needs is accepted: one disk, no ZIP64, no
//! encryption, stored or deflated entries, UTF-8 or ASCII names, and no
//! prepended data or overlapping entries. Every entry name is checked
//! before any byte is inflated: no absolute or drive paths, no `..`, no
//! empty or `.` segments, no Windows-invalid characters, device names or
//! trailing dots and spaces, no links, and no duplicates (ASCII and
//! Unicode case folded, and no file that is also a folder prefix). Sizes
//! and entry counts are bounded before inflation, inflation stops at the
//! declared size, and the CRC-32 must match.
//!
//! The reader works on bytes in memory and writes nothing; the caller
//! decides where entries go.

use std::collections::HashSet;

/// Bounds for one archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveLimits {
    pub max_entries: usize,
    /// Largest uncompressed entry (it is inflated in memory).
    pub max_entry_bytes: u64,
    /// Largest sum of uncompressed sizes (.NET `MaxDownloadBytes`).
    pub max_total_bytes: u64,
    /// Longest entry name, in bytes.
    pub max_name_bytes: usize,
    /// Deepest folder nesting.
    pub max_depth: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 4096,
            max_entry_bytes: 256 * 1024 * 1024,
            max_total_bytes: 1_000_000_000,
            max_name_bytes: 240,
            max_depth: 16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveError {
    /// Not a well-formed single-disk ZIP.
    Corrupt,
    /// ZIP64, encryption, a multi-disk archive or another method.
    Unsupported,
    /// An absolute, drive, empty, dot or Windows-invalid path.
    InvalidPath,
    /// A `..` segment.
    Traversal,
    TooLarge,
    TooManyEntries,
    /// Two entries name the same path (case folded), or a file is also a
    /// folder.
    Duplicate,
    /// A symbolic link or reparse point.
    Link,
    /// The inflated bytes do not match the recorded CRC-32 or size.
    Checksum,
}

impl ArchiveError {
    /// The diagnostics text. The first four are the .NET messages
    /// (`UpdateService.ExtractArchiveSafelyAsync`).
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            Self::InvalidPath => "The update archive contains an invalid path.",
            Self::Traversal => "The update archive contains a path traversal entry.",
            Self::TooLarge | Self::TooManyEntries => {
                "The update archive is larger than the supported limit."
            }
            Self::Corrupt | Self::Unsupported | Self::Checksum => {
                "The update archive is damaged or uses an unsupported format."
            }
            Self::Duplicate => "The update archive contains duplicate entries.",
            Self::Link => "The update archive contains a link.",
        }
    }
}

/// One checked entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Relative path with `/` separators; a folder has no trailing `/`.
    pub path: String,
    pub is_dir: bool,
    pub uncompressed_size: u64,
    method: u16,
    crc32: u32,
    data_start: usize,
    compressed_size: usize,
}

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const EOCD_LENGTH: usize = 22;
const CENTRAL_LENGTH: usize = 46;
const LOCAL_LENGTH: usize = 30;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATED: u16 = 8;
const FLAG_ENCRYPTED: u16 = 0x0001;
const FLAG_STRONG_ENCRYPTION: u16 = 0x0040;
const FLAG_UTF8: u16 = 0x0800;
const FLAG_MASKED_HEADER: u16 = 0x2000;

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, ArchiveError> {
    bytes
        .get(offset..offset + 2)
        .map(|slice| u16::from_le_bytes([slice[0], slice[1]]))
        .ok_or(ArchiveError::Corrupt)
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, ArchiveError> {
    bytes
        .get(offset..offset + 4)
        .map(|slice| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
        .ok_or(ArchiveError::Corrupt)
}

/// Lists and checks every entry. Nothing is inflated.
pub fn list(bytes: &[u8], limits: &ArchiveLimits) -> Result<Vec<Entry>, ArchiveError> {
    let eocd = find_end_of_central_directory(bytes)?;
    let disk = u16_at(bytes, eocd + 4)?;
    let central_disk = u16_at(bytes, eocd + 6)?;
    let entries_on_disk = u16_at(bytes, eocd + 8)?;
    let entries_total = u16_at(bytes, eocd + 10)?;
    let central_size = u32_at(bytes, eocd + 12)?;
    let central_offset = u32_at(bytes, eocd + 16)?;
    if entries_total == u16::MAX || central_size == u32::MAX || central_offset == u32::MAX {
        return Err(ArchiveError::Unsupported);
    }
    if disk != 0 || central_disk != 0 || entries_on_disk != entries_total {
        return Err(ArchiveError::Unsupported);
    }
    if usize::from(entries_total) > limits.max_entries {
        return Err(ArchiveError::TooManyEntries);
    }
    let central_offset = central_offset as usize;
    // No data before the first entry's header (checked below) and no gap
    // between the central directory and its end record.
    if central_offset.checked_add(central_size as usize) != Some(eocd) {
        return Err(ArchiveError::Corrupt);
    }
    let mut entries = Vec::with_capacity(usize::from(entries_total));
    let mut folded = HashSet::new();
    let mut total: u64 = 0;
    let mut cursor = central_offset;
    for _ in 0..entries_total {
        let entry = read_central_entry(bytes, &mut cursor, central_offset, limits, &mut total)?;
        if !folded.insert(fold(&entry.path)) {
            return Err(ArchiveError::Duplicate);
        }
        entries.push(entry);
    }
    if cursor != eocd {
        return Err(ArchiveError::Corrupt);
    }
    check_prefix_conflicts(&entries)?;
    check_layout(&entries, central_offset)?;
    Ok(entries)
}

fn find_end_of_central_directory(bytes: &[u8]) -> Result<usize, ArchiveError> {
    if bytes.len() < EOCD_LENGTH {
        return Err(ArchiveError::Corrupt);
    }
    let earliest = bytes
        .len()
        .saturating_sub(EOCD_LENGTH + usize::from(u16::MAX));
    let mut position = bytes.len() - EOCD_LENGTH;
    loop {
        if u32_at(bytes, position)? == EOCD_SIGNATURE {
            let comment = usize::from(u16_at(bytes, position + 20)?);
            // The record and its comment end the file exactly.
            if position + EOCD_LENGTH + comment == bytes.len() {
                return Ok(position);
            }
        }
        if position == earliest {
            return Err(ArchiveError::Corrupt);
        }
        position -= 1;
    }
}

fn read_central_entry(
    bytes: &[u8],
    cursor: &mut usize,
    central_offset: usize,
    limits: &ArchiveLimits,
    total: &mut u64,
) -> Result<Entry, ArchiveError> {
    let at = *cursor;
    if u32_at(bytes, at)? != CENTRAL_SIGNATURE {
        return Err(ArchiveError::Corrupt);
    }
    let made_by = u16_at(bytes, at + 4)?;
    let flags = u16_at(bytes, at + 8)?;
    let method = u16_at(bytes, at + 10)?;
    let crc32 = u32_at(bytes, at + 16)?;
    let compressed = u32_at(bytes, at + 20)?;
    let uncompressed = u32_at(bytes, at + 24)?;
    let name_length = usize::from(u16_at(bytes, at + 28)?);
    let extra_length = usize::from(u16_at(bytes, at + 30)?);
    let comment_length = usize::from(u16_at(bytes, at + 32)?);
    let disk_start = u16_at(bytes, at + 34)?;
    let external = u32_at(bytes, at + 38)?;
    let local_offset = u32_at(bytes, at + 42)?;
    let name_start = at + CENTRAL_LENGTH;
    let name = bytes
        .get(name_start..name_start + name_length)
        .ok_or(ArchiveError::Corrupt)?;
    *cursor = name_start + name_length + extra_length + comment_length;
    if *cursor > bytes.len() {
        return Err(ArchiveError::Corrupt);
    }
    if flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION | FLAG_MASKED_HEADER) != 0 {
        return Err(ArchiveError::Unsupported);
    }
    if compressed == u32::MAX || uncompressed == u32::MAX || local_offset == u32::MAX {
        return Err(ArchiveError::Unsupported);
    }
    if disk_start != 0 {
        return Err(ArchiveError::Unsupported);
    }
    if name_length > limits.max_name_bytes {
        return Err(ArchiveError::InvalidPath);
    }
    // Without the UTF-8 flag a name is in a code page: only ASCII has one
    // meaning.
    if flags & FLAG_UTF8 == 0 && !name.is_ascii() {
        return Err(ArchiveError::InvalidPath);
    }
    let name = std::str::from_utf8(name).map_err(|_| ArchiveError::InvalidPath)?;
    let (path, is_dir) = check_name(name, limits)?;
    let uncompressed_size = u64::from(uncompressed);
    if uncompressed_size > limits.max_entry_bytes
        || *total > limits.max_total_bytes.saturating_sub(uncompressed_size)
    {
        return Err(ArchiveError::TooLarge);
    }
    *total += uncompressed_size;
    if is_link(made_by, external) {
        return Err(ArchiveError::Link);
    }
    match method {
        METHOD_STORED if compressed == uncompressed => {}
        METHOD_DEFLATED => {}
        _ => return Err(ArchiveError::Unsupported),
    }
    if is_dir && uncompressed != 0 {
        return Err(ArchiveError::Corrupt);
    }
    let local = local_offset as usize;
    if local >= central_offset {
        return Err(ArchiveError::Corrupt);
    }
    if u32_at(bytes, local)? != LOCAL_SIGNATURE {
        return Err(ArchiveError::Corrupt);
    }
    let local_flags = u16_at(bytes, local + 6)?;
    let local_method = u16_at(bytes, local + 8)?;
    let local_name_length = usize::from(u16_at(bytes, local + 26)?);
    let local_extra_length = usize::from(u16_at(bytes, local + 28)?);
    let local_name = bytes
        .get(local + LOCAL_LENGTH..local + LOCAL_LENGTH + local_name_length)
        .ok_or(ArchiveError::Corrupt)?;
    if local_method != method
        || local_flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION) != 0
        || local_name != name.as_bytes()
    {
        return Err(ArchiveError::Corrupt);
    }
    let data_start = local + LOCAL_LENGTH + local_name_length + local_extra_length;
    let compressed_size = compressed as usize;
    match data_start.checked_add(compressed_size) {
        Some(end) if end <= central_offset => {}
        _ => return Err(ArchiveError::Corrupt),
    }
    Ok(Entry {
        path,
        is_dir,
        uncompressed_size,
        method,
        crc32,
        data_start,
        compressed_size,
    })
}

/// A Unix symbolic link (or any non-regular, non-folder Unix type), or a
/// Windows reparse point.
fn is_link(made_by: u16, external: u32) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const UNIX: u16 = 3;
    let unix_type = (external >> 16) & 0o170_000;
    let unix_special =
        made_by >> 8 == UNIX && unix_type != 0 && unix_type != 0o100_000 && unix_type != 0o040_000;
    unix_special || external & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Checks one entry name; returns the path without a trailing `/`.
fn check_name(name: &str, limits: &ArchiveLimits) -> Result<(String, bool), ArchiveError> {
    if name.is_empty() || name.starts_with('/') || name.contains([':', '\\']) {
        return Err(ArchiveError::InvalidPath);
    }
    let (body, is_dir) = match name.strip_suffix('/') {
        Some(body) => (body, true),
        None => (name, false),
    };
    let segments: Vec<&str> = body.split('/').collect();
    if segments.contains(&"..") {
        return Err(ArchiveError::Traversal);
    }
    if segments.len() > limits.max_depth {
        return Err(ArchiveError::InvalidPath);
    }
    for segment in &segments {
        if !valid_segment(segment) {
            return Err(ArchiveError::InvalidPath);
        }
    }
    Ok((body.to_owned(), is_dir))
}

fn valid_segment(segment: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if segment.is_empty() || segment == "." || segment.ends_with(['.', ' ']) {
        return false;
    }
    if segment
        .chars()
        .any(|ch| ch < ' ' || matches!(ch, '<' | '>' | '"' | '|' | '?' | '*' | '\u{7f}'))
    {
        return false;
    }
    let stem = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_lowercase();
    !RESERVED.contains(&stem.as_str())
}

/// Case-folded form for duplicate detection.
fn fold(path: &str) -> String {
    path.to_lowercase()
}

fn check_prefix_conflicts(entries: &[Entry]) -> Result<(), ArchiveError> {
    let files: HashSet<String> = entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(|entry| fold(&entry.path))
        .collect();
    for entry in entries {
        let folded = fold(&entry.path);
        let mut parent = folded.as_str();
        while let Some((head, _)) = parent.rsplit_once('/') {
            if files.contains(head) {
                return Err(ArchiveError::Duplicate);
            }
            parent = head;
        }
    }
    Ok(())
}

/// Entry data must not overlap (no shared or nested local records).
fn check_layout(entries: &[Entry], central_offset: usize) -> Result<(), ArchiveError> {
    let mut spans: Vec<(usize, usize)> = entries
        .iter()
        .map(|entry| (entry.data_start, entry.data_start + entry.compressed_size))
        .collect();
    spans.sort_unstable();
    for pair in spans.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(ArchiveError::Corrupt);
        }
    }
    if spans.last().is_some_and(|span| span.1 > central_offset) {
        return Err(ArchiveError::Corrupt);
    }
    Ok(())
}

/// Inflates one listed entry, bounded by its declared size, and checks
/// its size and CRC-32.
pub fn read(bytes: &[u8], entry: &Entry) -> Result<Vec<u8>, ArchiveError> {
    let data = bytes
        .get(entry.data_start..entry.data_start + entry.compressed_size)
        .ok_or(ArchiveError::Corrupt)?;
    let expected = usize::try_from(entry.uncompressed_size).map_err(|_| ArchiveError::TooLarge)?;
    let content = match entry.method {
        METHOD_STORED => data.to_vec(),
        METHOD_DEFLATED => miniz_oxide::inflate::decompress_to_vec_with_limit(data, expected)
            .map_err(|error| {
                if matches!(
                    error.status,
                    miniz_oxide::inflate::TINFLStatus::HasMoreOutput
                ) {
                    ArchiveError::TooLarge
                } else {
                    ArchiveError::Corrupt
                }
            })?,
        _ => return Err(ArchiveError::Unsupported),
    };
    if content.len() != expected || crc32(&content) != entry.crc32 {
        return Err(ArchiveError::Checksum);
    }
    Ok(content)
}

/// CRC-32 (IEEE 802.3, reflected), as ZIP records it.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    const fn table() -> [u32; 256] {
        let mut table = [0_u32; 256];
        let mut index = 0;
        while index < 256 {
            let mut value = index as u32;
            let mut bit = 0;
            while bit < 8 {
                value = if value & 1 == 1 {
                    0xEDB8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
                bit += 1;
            }
            table[index] = value;
            index += 1;
        }
        table
    }
    const TABLE: [u32; 256] = table();
    !bytes.iter().fold(u32::MAX, |crc, byte| {
        TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_reference_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn names_are_checked_before_anything_else() {
        let limits = ArchiveLimits::default();
        for (name, expected) in [
            ("../escaped.txt", ArchiveError::Traversal),
            ("a/../../b", ArchiveError::Traversal),
            ("/abs.txt", ArchiveError::InvalidPath),
            ("C:/x.txt", ArchiveError::InvalidPath),
            ("a\\b.txt", ArchiveError::InvalidPath),
            ("file.txt:stream", ArchiveError::InvalidPath),
            ("a//b", ArchiveError::InvalidPath),
            ("./a", ArchiveError::InvalidPath),
            ("a/./b", ArchiveError::InvalidPath),
            ("nul", ArchiveError::InvalidPath),
            ("CON.txt", ArchiveError::InvalidPath),
            ("dir/LPT1", ArchiveError::InvalidPath),
            ("trailing.", ArchiveError::InvalidPath),
            ("trailing ", ArchiveError::InvalidPath),
            ("a?b", ArchiveError::InvalidPath),
            ("a\u{1}b", ArchiveError::InvalidPath),
            ("", ArchiveError::InvalidPath),
        ] {
            assert_eq!(check_name(name, &limits), Err(expected), "{name:?}");
        }
        let deep = vec!["d"; 17].join("/");
        assert_eq!(check_name(&deep, &limits), Err(ArchiveError::InvalidPath));
        assert_eq!(
            check_name("docs/readme.txt", &limits),
            Ok(("docs/readme.txt".to_owned(), false))
        );
        assert_eq!(check_name("docs/", &limits), Ok(("docs".to_owned(), true)));
        assert_eq!(
            check_name("console.txt", &limits),
            Ok(("console.txt".to_owned(), false))
        );
    }
}
