//! The embedded payload and its verification.
//!
//! A [`VerifiedPayload`] exists only after [`VerifiedPayload::verify`]
//! checked the container (trailer and per-file SHA-256, see
//! [`crate::container`]) and the exact file set. The installer takes a
//! `&VerifiedPayload`, so nothing is written before the hashes passed.

use std::fmt;

use crate::container::{self, ContainerError, Entry};
use crate::names::FileNames;

/// The container that build.rs wrote. Empty unless CI set
/// `MARKNEXIA_SETUP_PAYLOAD_DIR`.
static EMBEDDED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));

#[must_use]
pub fn embedded() -> &'static [u8] {
    EMBEDDED
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayloadError {
    /// No container was embedded (a developer build).
    Missing,
    Container(ContainerError),
    /// The container is valid but holds other files.
    UnexpectedFileSet(Vec<String>),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter
                .write_str("This setup executable does not contain an application payload."),
            Self::Container(error) => {
                write!(formatter, "The setup payload failed verification: {error}.")
            }
            Self::UnexpectedFileSet(names) => write!(
                formatter,
                "The setup payload failed verification: it holds {} instead of the expected files.",
                names.join(", ")
            ),
        }
    }
}

impl std::error::Error for PayloadError {}

/// A payload whose every hash and whose file set were checked.
#[derive(Debug)]
pub struct VerifiedPayload<'a> {
    entries: Vec<Entry<'a>>,
    container_sha256: [u8; 32],
}

impl<'a> VerifiedPayload<'a> {
    /// Checks `bytes` completely against the payload of `names` (the
    /// executable passes [`FileNames::release`]: the portable ZIP's four
    /// files plus the SPDX SBOM that describes this setup).
    ///
    /// # Errors
    ///
    /// [`PayloadError`] when the container is empty, damaged, or holds a
    /// different file set.
    pub fn verify(bytes: &'a [u8], names: &FileNames) -> Result<Self, PayloadError> {
        if bytes.is_empty() {
            return Err(PayloadError::Missing);
        }
        let entries = container::decode(bytes).map_err(PayloadError::Container)?;
        let found: Vec<String> = entries.iter().map(|entry| entry.name.to_owned()).collect();
        if found.as_slice() != names.payload() {
            return Err(PayloadError::UnexpectedFileSet(found));
        }
        use sha2::Digest;
        let container_sha256: [u8; 32] = sha2::Sha256::digest(bytes).into();
        Ok(Self {
            entries,
            container_sha256,
        })
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry<'a>] {
        &self.entries
    }

    /// Bytes on disk after install, without the setup copy.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| entry.bytes.len() as u64)
            .sum()
    }

    /// The `--verify` report: what this setup would install.
    #[must_use]
    pub fn report(&self, version: &str) -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": "marknexia-rust-setup-payload-v1",
            "product": crate::identity::PRODUCT_NAME,
            "version": version,
            "containerSha256": container::hex(&self.container_sha256),
            "files": self.entries.iter().map(|entry| serde_json::json!({
                "name": entry.name,
                "bytes": entry.bytes.len(),
                "sha256": container::hex(&entry.sha256),
            })).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Text stand-ins for every payload file, under the non-executable
    /// stand-in names ([`FileNames::stand_ins`]).
    pub(crate) fn sample_files() -> Vec<(String, Vec<u8>)> {
        FileNames::stand_ins()
            .payload()
            .iter()
            .map(|name| {
                (
                    (*name).to_owned(),
                    format!("text stand-in for {name}\n").into_bytes(),
                )
            })
            .collect()
    }

    pub(crate) fn sample_container() -> Vec<u8> {
        container::encode(&sample_files()).expect("encode")
    }

    #[test]
    fn a_developer_build_has_no_payload() {
        // Local builds never set MARKNEXIA_SETUP_PAYLOAD_DIR.
        if std::env::var_os("MARKNEXIA_SETUP_PAYLOAD_DIR").is_none() {
            assert!(embedded().is_empty());
            assert_eq!(
                VerifiedPayload::verify(embedded(), &FileNames::release()).unwrap_err(),
                PayloadError::Missing
            );
            assert_eq!(
                PayloadError::Missing.to_string(),
                "This setup executable does not contain an application payload."
            );
        }
    }

    #[test]
    fn a_complete_payload_verifies_and_reports_its_hashes() {
        let bytes = sample_container();
        let payload = VerifiedPayload::verify(&bytes, &FileNames::stand_ins()).expect("verify");
        let names: Vec<&str> = payload.entries().iter().map(|entry| entry.name).collect();
        assert_eq!(names, FileNames::stand_ins().payload());
        let report = payload.report("0.1.0");
        assert_eq!(report["files"].as_array().map(Vec::len), Some(5));
        assert_eq!(report["version"], "0.1.0");
        use sha2::Digest;
        let expected: [u8; 32] = sha2::Sha256::digest(b"text stand-in for README.txt\n").into();
        assert_eq!(report["files"][0]["sha256"], container::hex(&expected));
        assert_eq!(
            payload.total_bytes(),
            sample_files()
                .iter()
                .map(|(_, bytes)| bytes.len() as u64)
                .sum::<u64>()
        );
    }

    #[test]
    fn a_missing_or_extra_file_is_refused() {
        let mut files = sample_files();
        files.pop();
        let bytes = container::encode(&files).expect("encode");
        assert!(matches!(
            VerifiedPayload::verify(&bytes, &FileNames::stand_ins()),
            Err(PayloadError::UnexpectedFileSet(_))
        ));
        let mut files = sample_files();
        files.push(("extra.txt".to_owned(), b"x".to_vec()));
        let bytes = container::encode(&files).expect("encode");
        assert!(matches!(
            VerifiedPayload::verify(&bytes, &FileNames::stand_ins()),
            Err(PayloadError::UnexpectedFileSet(_))
        ));
    }

    #[test]
    fn the_release_file_set_is_checked_in_memory() {
        // Real names only in memory: this container is never written to disk.
        let release: Vec<(String, Vec<u8>)> = FileNames::release()
            .payload()
            .iter()
            .map(|name| (name.clone(), b"in-memory text".to_vec()))
            .collect();
        let bytes = container::encode(&release).expect("encode");
        assert!(VerifiedPayload::verify(&bytes, &FileNames::release()).is_ok());
        assert!(matches!(
            VerifiedPayload::verify(&bytes, &FileNames::stand_ins()),
            Err(PayloadError::UnexpectedFileSet(_))
        ));
    }

    #[test]
    fn any_changed_byte_is_refused() {
        let original = sample_container();
        for index in (0..original.len()).step_by(7) {
            let mut bytes = original.clone();
            bytes[index] ^= 0x01;
            assert!(
                VerifiedPayload::verify(&bytes, &FileNames::stand_ins()).is_err(),
                "byte {index}"
            );
        }
    }
}
