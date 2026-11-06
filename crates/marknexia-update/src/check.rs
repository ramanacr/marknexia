//! "Check for updates": fetch and verify the manifest, and the shell's
//! .NET-parity texts (`MainWindow.CheckForUpdates_Click`).

use std::sync::atomic::AtomicBool;

use crate::{
    config::UpdateConfig,
    manifest::{self, Architecture, Expectations, Manifest, ManifestError},
    stage::{FetchError, Fetcher},
    version::Version,
};

/// Largest artifact accepted (the in-memory download bound).
pub const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

pub const UP_TO_DATE_TITLE: &str = "Marknexia is up to date";
pub const AVAILABLE_TITLE: &str = "Marknexia update available";
pub const DOWNLOAD_AND_RESTART: &str = "Download and restart";
pub const LATER: &str = "Later";
pub const DOWNLOADING_NOTICE: &str = "Downloading and verifying the Marknexia update\u{2026}";
/// The .NET packaged-installation text, without the version.
pub const PACKAGED_MESSAGE: &str = "This installation is managed as an MSIX package; use the official release page or Store channel to update it.";

#[must_use]
pub fn up_to_date_text(current: &str) -> String {
    format!("You are using Marknexia {current}.")
}

#[must_use]
pub fn available_text(version: &Version) -> String {
    format!(
        "Version {version} is available. Marknexia will verify the release signature and checksum, install it safely, and restart."
    )
}

#[must_use]
pub fn unavailable_notice(error: &str) -> String {
    format!("Update check unavailable: {error}")
}

#[must_use]
pub fn install_failed_notice(error: &str) -> String {
    format!("Update could not be installed: {error}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    UpToDate,
    Available(Manifest),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckError {
    Download(FetchError),
    Manifest(ManifestError),
    InvalidCurrentVersion,
}

impl CheckError {
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Download(error) => error.message(),
            Self::Manifest(ManifestError::OsTooOld) => {
                "The latest version needs a newer version of Windows.".to_owned()
            }
            Self::Manifest(_) => {
                "The update information is not valid or is not signed for this build.".to_owned()
            }
            Self::InvalidCurrentVersion => "Release version is not comparable.".to_owned(),
        }
    }
}

/// Fetches the manifest and its signature and verifies them. A verified
/// manifest that is not newer (a replay or downgrade) means up to date.
pub fn check_for_update(
    config: &UpdateConfig,
    fetcher: &dyn Fetcher,
    current_version: &str,
    architecture: Architecture,
    os_build: u32,
    cancel: &AtomicBool,
) -> Result<CheckOutcome, CheckError> {
    let current = Version::parse(current_version).map_err(|_| CheckError::InvalidCurrentVersion)?;
    let manifest_bytes = fetcher
        .fetch(
            &config.manifest_url,
            manifest::MAX_MANIFEST_BYTES as u64,
            cancel,
        )
        .map_err(CheckError::Download)?;
    let signature_bytes = fetcher
        .fetch(
            &config.signature_url,
            manifest::MAX_SIGNATURE_BYTES as u64,
            cancel,
        )
        .map_err(CheckError::Download)?;
    let expect = Expectations {
        current_version: &current,
        architecture,
        os_build,
        trusted_hosts: &config.allowed_hosts,
        max_byte_length: MAX_ARTIFACT_BYTES,
    };
    match manifest::verify(&manifest_bytes, &signature_bytes, &config.keys, &expect) {
        Ok(manifest) => Ok(CheckOutcome::Available(manifest)),
        Err(ManifestError::NotNewer) => Ok(CheckOutcome::UpToDate),
        Err(error) => Err(CheckError::Manifest(error)),
    }
}
