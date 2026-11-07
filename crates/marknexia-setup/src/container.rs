//! The embedded payload container (`MNXSETUP-PAYLD-1`).
//!
//! build.rs includes this file with `#[path]` to write the container, so it
//! uses only `std` and `sha2`, never `crate::`.
//!
//! ```text
//! magic            16 bytes  "MNXSETUP-PAYLD-1"
//! count            u32 LE    number of files
//! count entries:
//!   name length    u16 LE
//!   name           ASCII file name (no folders), strictly ascending (ordinal)
//!   size           u64 LE
//!   sha256         32 bytes  SHA-256 of the file bytes
//! data             the file bytes, in entry order, without padding
//! trailer          32 bytes  SHA-256 of every byte before the trailer
//! ```
//!
//! [`decode`] checks the whole container before it returns a slice: the
//! trailer, every entry hash, the exact total length, the name rules and
//! the limits. A container that fails any check is refused.

use sha2::{Digest, Sha256};
use std::fmt;

pub const MAGIC: [u8; 16] = *b"MNXSETUP-PAYLD-1";
/// At most this many files.
pub const MAX_FILES: usize = 16;
/// At most this many bytes in one container (files, header and trailer).
pub const MAX_CONTAINER_BYTES: u64 = 64 * 1024 * 1024;
/// At most this many bytes in a file name.
pub const MAX_NAME_BYTES: usize = 64;
const DIGEST_BYTES: usize = 32;

/// One file of a decoded container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    pub name: &'a str,
    pub sha256: [u8; 32],
    pub bytes: &'a [u8],
}

/// Why a container was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerError {
    Empty,
    BadMagic,
    Truncated,
    TooLarge,
    TooManyFiles(usize),
    InvalidName(String),
    NamesNotAscending(String),
    EntryHashMismatch(String),
    TrailerMismatch,
    TrailingBytes,
}

impl fmt::Display for ContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("the payload is empty"),
            Self::BadMagic => formatter.write_str("the payload has an unknown format"),
            Self::Truncated => formatter.write_str("the payload is truncated"),
            Self::TooLarge => formatter.write_str("the payload is larger than the supported limit"),
            Self::TooManyFiles(count) => write!(formatter, "the payload lists {count} files"),
            Self::InvalidName(name) => {
                write!(formatter, "the payload file name '{name}' is not allowed")
            }
            Self::NamesNotAscending(name) => {
                write!(
                    formatter,
                    "the payload file '{name}' is duplicated or out of order"
                )
            }
            Self::EntryHashMismatch(name) => {
                write!(
                    formatter,
                    "the payload file '{name}' does not match its SHA-256"
                )
            }
            Self::TrailerMismatch => formatter.write_str("the payload does not match its SHA-256"),
            Self::TrailingBytes => formatter.write_str("the payload has unexpected trailing bytes"),
        }
    }
}

impl std::error::Error for ContainerError {}

/// A plain file name: 1 to 64 bytes of ASCII letters, digits, `.`, `-` and
/// `_`, not starting with a dot. No folders, no device or drive syntax.
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// Writes a container. Files are sorted by name (ordinal).
///
/// # Errors
///
/// Refuses invalid or duplicate names, more than [`MAX_FILES`] files and a
/// container over [`MAX_CONTAINER_BYTES`].
pub fn encode(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, ContainerError> {
    if files.is_empty() {
        return Err(ContainerError::Empty);
    }
    if files.len() > MAX_FILES {
        return Err(ContainerError::TooManyFiles(files.len()));
    }
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    for pair in sorted.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(ContainerError::NamesNotAscending(pair[1].0.clone()));
        }
    }
    let mut total: u64 = (MAGIC.len() + 4 + DIGEST_BYTES) as u64;
    for (name, bytes) in &sorted {
        if !is_valid_name(name) {
            return Err(ContainerError::InvalidName(name.clone()));
        }
        total = total
            .saturating_add((2 + name.len() + 8 + DIGEST_BYTES) as u64)
            .saturating_add(bytes.len() as u64);
    }
    if total > MAX_CONTAINER_BYTES {
        return Err(ContainerError::TooLarge);
    }
    let mut out = Vec::with_capacity(usize::try_from(total).map_err(|_| ContainerError::TooLarge)?);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(sorted.len() as u32).to_le_bytes());
    for (name, bytes) in &sorted {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        out.extend_from_slice(&digest);
    }
    for (_, bytes) in &sorted {
        out.extend_from_slice(bytes);
    }
    let trailer: [u8; 32] = Sha256::digest(&out).into();
    out.extend_from_slice(&trailer);
    Ok(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], ContainerError> {
        let end = self
            .position
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(ContainerError::Truncated)?;
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ContainerError> {
        let mut out = [0_u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
}

/// Reads and checks a container.
///
/// # Errors
///
/// See [`ContainerError`]. Nothing is returned unless every check passed.
pub fn decode(bytes: &[u8]) -> Result<Vec<Entry<'_>>, ContainerError> {
    if bytes.is_empty() {
        return Err(ContainerError::Empty);
    }
    if bytes.len() as u64 > MAX_CONTAINER_BYTES {
        return Err(ContainerError::TooLarge);
    }
    if bytes.len() < MAGIC.len() + 4 + DIGEST_BYTES {
        return Err(ContainerError::Truncated);
    }
    if bytes[..MAGIC.len()] != MAGIC {
        return Err(ContainerError::BadMagic);
    }
    // The trailer first: a damaged container is refused as a whole.
    let (body, trailer) = bytes.split_at(bytes.len() - DIGEST_BYTES);
    let computed: [u8; 32] = Sha256::digest(body).into();
    if computed.as_slice() != trailer {
        return Err(ContainerError::TrailerMismatch);
    }
    let mut reader = Reader {
        bytes: body,
        position: MAGIC.len(),
    };
    let count = u32::from_le_bytes(reader.array::<4>()?) as usize;
    if count == 0 {
        return Err(ContainerError::Empty);
    }
    if count > MAX_FILES {
        return Err(ContainerError::TooManyFiles(count));
    }
    let mut headers = Vec::with_capacity(count);
    let mut previous: Option<&str> = None;
    for _ in 0..count {
        let length = u16::from_le_bytes(reader.array::<2>()?) as usize;
        let raw = reader.take(length)?;
        let name = std::str::from_utf8(raw)
            .ok()
            .filter(|name| is_valid_name(name))
            .ok_or_else(|| {
                ContainerError::InvalidName(String::from_utf8_lossy(raw).into_owned())
            })?;
        if previous.is_some_and(|previous| previous.as_bytes() >= name.as_bytes()) {
            return Err(ContainerError::NamesNotAscending(name.to_owned()));
        }
        previous = Some(name);
        let size = u64::from_le_bytes(reader.array::<8>()?);
        let sha256 = reader.array::<32>()?;
        headers.push((name, size, sha256));
    }
    let mut entries = Vec::with_capacity(count);
    for (name, size, sha256) in headers {
        let size = usize::try_from(size).map_err(|_| ContainerError::TooLarge)?;
        let data = reader.take(size)?;
        let digest: [u8; 32] = Sha256::digest(data).into();
        if digest != sha256 {
            return Err(ContainerError::EntryHashMismatch(name.to_owned()));
        }
        entries.push(Entry {
            name,
            sha256,
            bytes: data,
        });
    }
    if reader.position != body.len() {
        return Err(ContainerError::TrailingBytes);
    }
    Ok(entries)
}

/// Lower-case hex of a digest.
#[must_use]
pub fn hex(digest: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![
            ("b.txt".to_owned(), b"second".to_vec()),
            ("A.txt".to_owned(), b"first".to_vec()),
            ("empty.txt".to_owned(), Vec::new()),
        ]
    }

    #[test]
    fn round_trip_sorts_names_ordinally() {
        let bytes = encode(&files()).expect("encode");
        let entries = decode(&bytes).expect("decode");
        let names: Vec<&str> = entries.iter().map(|entry| entry.name).collect();
        assert_eq!(names, ["A.txt", "b.txt", "empty.txt"]);
        assert_eq!(entries[0].bytes, b"first");
        assert_eq!(entries[2].bytes, b"");
        let digest: [u8; 32] = Sha256::digest(b"second").into();
        assert_eq!(entries[1].sha256, digest);
    }

    #[test]
    fn encoding_is_deterministic() {
        let mut reversed = files();
        reversed.reverse();
        assert_eq!(encode(&files()), encode(&reversed));
    }

    #[test]
    fn names_are_plain_file_names() {
        for name in [
            "a.txt",
            "marknexia-win32.exe",
            "THIRD-PARTY-NOTICES.txt",
            "x_y",
        ] {
            assert!(is_valid_name(name), "{name}");
        }
        for name in [
            "",
            ".hidden",
            "trailing.",
            "a/b",
            "a\\b",
            "..",
            "c:x",
            "a b",
            "é",
            "a\0",
            &"x".repeat(65),
        ] {
            assert!(!is_valid_name(name), "{name:?}");
        }
        let error = encode(&[("../x".to_owned(), Vec::new())]).unwrap_err();
        assert_eq!(error, ContainerError::InvalidName("../x".to_owned()));
    }

    #[test]
    fn duplicates_and_empty_inputs_are_refused() {
        let duplicate = vec![
            ("a.txt".to_owned(), Vec::new()),
            ("a.txt".to_owned(), Vec::new()),
        ];
        assert!(matches!(
            encode(&duplicate),
            Err(ContainerError::NamesNotAscending(_))
        ));
        assert_eq!(encode(&[]), Err(ContainerError::Empty));
        assert_eq!(decode(&[]), Err(ContainerError::Empty));
    }

    #[test]
    fn damage_anywhere_is_detected() {
        let original = encode(&files()).expect("encode");
        for index in 0..original.len() {
            let mut bytes = original.clone();
            bytes[index] ^= 0x80;
            assert!(decode(&bytes).is_err(), "byte {index}");
        }
        assert!(decode(&original[..original.len() - 1]).is_err());
        let mut longer = original.clone();
        longer.push(0);
        assert!(decode(&longer).is_err());
    }

    /// A container whose trailer is recomputed after a header change still
    /// fails on the entry hash, the order or the length.
    fn reseal(mut body: Vec<u8>) -> Vec<u8> {
        let trailer: [u8; 32] = Sha256::digest(&body).into();
        body.extend_from_slice(&trailer);
        body
    }

    #[test]
    fn a_resealed_but_inconsistent_container_is_refused() {
        let original = encode(&files()).expect("encode");
        let body = original[..original.len() - 32].to_vec();

        // Change one data byte: the entry hash no longer matches.
        let mut changed = body.clone();
        let last = changed.len() - 1;
        changed[last] ^= 0x01;
        assert!(matches!(
            decode(&reseal(changed)),
            Err(ContainerError::EntryHashMismatch(_))
        ));

        // Extra bytes after the data.
        let mut extra = body.clone();
        extra.push(b'x');
        assert_eq!(decode(&reseal(extra)), Err(ContainerError::TrailingBytes));

        // A count that is too large.
        let mut many = body.clone();
        many[16..20].copy_from_slice(&(MAX_FILES as u32 + 1).to_le_bytes());
        assert!(matches!(
            decode(&reseal(many)),
            Err(ContainerError::TooManyFiles(_))
        ));

        // Names out of order: swap the first letters "A" and "b".
        let mut swapped = body;
        let first = 20
            + swapped[20..]
                .iter()
                .position(|byte| *byte == b'A')
                .expect("A");
        swapped[first] = b'c';
        assert_eq!(
            decode(&reseal(swapped)),
            Err(ContainerError::NamesNotAscending("b.txt".to_owned()))
        );
    }

    #[test]
    fn a_wrong_magic_is_refused() {
        let mut bytes = encode(&files()).expect("encode");
        bytes[0] = b'X';
        let body = bytes[..bytes.len() - 32].to_vec();
        assert_eq!(decode(&reseal(body)), Err(ContainerError::BadMagic));
    }

    #[test]
    fn hex_is_lower_case() {
        assert_eq!(hex(&[0x00, 0xab, 0x7f]), "00ab7f");
    }
}
