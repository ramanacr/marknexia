//! Download, verification and side-by-side staging of a verified manifest's
//! portable ZIP.
//!
//! Order: download (bounded by `byteLength`) into memory; exact length;
//! SHA-256; list and check every ZIP entry and the payload policy; only
//! then write the entries into a new owner-only `staging\pending-<random>`
//! folder; check each allowed executable (PE machine and Authenticode
//! publisher) on disk; and finally rename the folder to
//! `versions\<version>-<arch>`. Any failure removes the pending folder and
//! leaves every other folder (stale stages included, as in .NET) alone.
//! The active version is never touched here.

use std::{
    fs::OpenOptions,
    hash::BuildHasher,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::{
    archive::{self, ArchiveError, ArchiveLimits},
    layout::{Layout, payload_id},
    manifest::{HttpsUrl, Manifest},
    payload::{self, AllowedExecutable, PayloadError, PayloadPolicy, PublisherPin},
    version::Version,
};

/// Why a download failed (mapped from the WinHTTP adapter).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchError {
    Offline,
    TimedOut,
    Cancelled,
    /// A redirect left the allowlist or the URL was refused.
    UntrustedLocation,
    TooManyRedirects,
    Http(u16),
    /// Declared or received length above the limit.
    TooLarge,
    /// The body ended before its declared length (an interrupted download).
    Truncated,
    Network(u32),
}

impl FetchError {
    #[must_use]
    pub fn message(self) -> String {
        match self {
            Self::Offline => "The update server could not be reached.".to_owned(),
            Self::TimedOut => "The update server did not respond in time.".to_owned(),
            Self::Cancelled => "The update was cancelled.".to_owned(),
            Self::UntrustedLocation => "Update download host is not trusted.".to_owned(),
            Self::TooManyRedirects => "The update server returned too many redirects.".to_owned(),
            Self::Http(status) => {
                format!("The update server returned HTTP status {status}.")
            }
            Self::TooLarge => "The update package is larger than the supported limit.".to_owned(),
            Self::Truncated => "The update download was interrupted.".to_owned(),
            Self::Network(code) => format!("The update download failed (error {code})."),
        }
    }
}

/// Fetches one HTTPS resource. Implementations stay on the allowlist for
/// every redirect, never return more than `max_bytes`, apply timeouts, and
/// stop with [`FetchError::Cancelled`] once `cancel` is set.
pub trait Fetcher {
    fn fetch(
        &self,
        url: &HttpsUrl,
        max_bytes: u64,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, FetchError>;
}

/// The platform operations staging needs.
pub trait StagePlatform {
    /// Creates `path` with a protected DACL that grants its owner only;
    /// fails if it exists.
    fn create_private_dir(&self, path: &Path) -> std::io::Result<()>;
    /// An existing folder (for example one left by the .NET updater): fails
    /// for a reparse point or a folder another account owns; otherwise
    /// re-applies the protected owner-only DACL.
    fn secure_existing_dir(&self, path: &Path) -> std::io::Result<()>;
    /// Checks a staged executable: PE machine `machine`, and the
    /// Authenticode signer required by `executable.signer` (the pinned
    /// `publisher` for [`payload::Signer::Release`]).
    fn check_executable(
        &self,
        path: &Path,
        executable: &AllowedExecutable,
        machine: u16,
        publisher: &PublisherPin,
    ) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageError {
    Download(FetchError),
    LengthMismatch,
    ChecksumMismatch,
    Archive(ArchiveError),
    Payload(PayloadError),
    /// An executable failed its PE or Authenticode check.
    Executable {
        name: String,
        reason: String,
    },
    Io(std::io::ErrorKind),
    Cancelled,
}

impl StageError {
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Download(error) => error.message(),
            Self::LengthMismatch => {
                "The downloaded update does not have the length its manifest declares.".to_owned()
            }
            // .NET `VerifySha256Async`.
            Self::ChecksumMismatch => "The downloaded update checksum does not match.".to_owned(),
            Self::Archive(error) => error.message().to_owned(),
            Self::Payload(error) => error.message(),
            Self::Executable { name, reason } => {
                format!("The update file '{name}' failed verification: {reason}")
            }
            Self::Io(kind) => format!("The update could not be saved ({kind})."),
            Self::Cancelled => "The update was cancelled.".to_owned(),
        }
    }
}

impl From<std::io::Error> for StageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}

/// A verified payload in `versions\<id>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedPayload {
    pub version: Version,
    pub id: String,
    pub directory: PathBuf,
    /// The shell executable to start for the health check.
    pub shell: PathBuf,
}

/// What to stage and how.
pub struct StageRequest<'a> {
    pub manifest: &'a Manifest,
    pub policy: &'a PayloadPolicy,
    pub limits: ArchiveLimits,
    pub publisher: &'a PublisherPin,
}

/// Downloads the artifact, then [`stage_bytes`].
pub fn download_and_stage(
    request: &StageRequest<'_>,
    layout: &Layout,
    fetcher: &dyn Fetcher,
    platform: &dyn StagePlatform,
    cancel: &AtomicBool,
) -> Result<StagedPayload, StageError> {
    let bytes = fetcher
        .fetch(&request.manifest.url, request.manifest.byte_length, cancel)
        .map_err(|error| match error {
            FetchError::Cancelled => StageError::Cancelled,
            other => StageError::Download(other),
        })?;
    stage_bytes(&bytes, request, layout, platform, cancel)
}

/// Verifies downloaded bytes against the manifest and stages them.
pub fn stage_bytes(
    bytes: &[u8],
    request: &StageRequest<'_>,
    layout: &Layout,
    platform: &dyn StagePlatform,
    cancel: &AtomicBool,
) -> Result<StagedPayload, StageError> {
    let manifest = request.manifest;
    if bytes.len() as u64 != manifest.byte_length {
        return Err(StageError::LengthMismatch);
    }
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    if !crate::hex::constant_time_eq(&digest, &manifest.sha256) {
        return Err(StageError::ChecksumMismatch);
    }
    let entries = archive::list(bytes, &request.limits).map_err(StageError::Archive)?;
    payload::check(&entries, request.policy).map_err(StageError::Payload)?;
    if cancel.load(Ordering::Acquire) {
        return Err(StageError::Cancelled);
    }
    ensure_private_dir(platform, layout.root())?;
    ensure_private_dir(platform, &layout.staging())?;
    ensure_private_dir(platform, &layout.versions())?;
    let pending = create_pending(platform, &layout.staging())?;
    let staged = (|| {
        extract(bytes, &entries, &pending, cancel)?;
        for executable in &request.policy.executables {
            let path = pending.join(&executable.name);
            if !path.is_file() {
                continue;
            }
            platform
                .check_executable(
                    &path,
                    executable,
                    manifest.architecture.pe_machine(),
                    request.publisher,
                )
                .map_err(|reason| StageError::Executable {
                    name: executable.name.clone(),
                    reason,
                })?;
        }
        if cancel.load(Ordering::Acquire) {
            return Err(StageError::Cancelled);
        }
        let id = payload_id(&manifest.version, manifest.architecture);
        let target = layout.payload(&id);
        // A folder left by an earlier failed attempt for this version is
        // never the active or rollback payload (the version is newer than
        // the running one), so it is replaced.
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        std::fs::rename(&pending, &target)?;
        let shell = request
            .policy
            .shell()
            .map(|shell| target.join(&shell.name))
            .unwrap_or_default();
        Ok(StagedPayload {
            version: manifest.version.clone(),
            id,
            directory: target,
            shell,
        })
    })();
    if staged.is_err() {
        let _ = std::fs::remove_dir_all(&pending);
    }
    staged
}

/// Creates `path` (and its parents) as an owner-only folder, or checks and
/// secures an existing one (`StagePlatform::secure_existing_dir`).
pub fn ensure_private_dir(platform: &dyn StagePlatform, path: &Path) -> Result<(), StageError> {
    // `symlink_metadata` does not follow a link: a planted link or
    // junction is passed to the platform check, which refuses it.
    if std::fs::symlink_metadata(path).is_ok() {
        return platform.secure_existing_dir(path).map_err(Into::into);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match platform.create_private_dir(path) {
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            platform.secure_existing_dir(path).map_err(Into::into)
        }
        other => other.map_err(Into::into),
    }
}

/// A new `pending-<random>` folder; an existing name is never reused.
fn create_pending(platform: &dyn StagePlatform, staging: &Path) -> Result<PathBuf, StageError> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    for _ in 0..8 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let token = std::collections::hash_map::RandomState::new().hash_one((
            std::process::id(),
            nanos,
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let folder = staging.join(format!("pending-{token:016x}"));
        match platform.create_private_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(StageError::Io(std::io::ErrorKind::AlreadyExists))
}

/// Writes every checked entry under `destination` (create-new, never
/// overwriting). Folders inherit the pending folder's DACL.
fn extract(
    bytes: &[u8],
    entries: &[archive::Entry],
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<(), StageError> {
    for entry in entries {
        if cancel.load(Ordering::Acquire) {
            return Err(StageError::Cancelled);
        }
        let target = entry
            .path
            .split('/')
            .fold(destination.to_path_buf(), |path, segment| {
                path.join(segment)
            });
        if entry.is_dir {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = archive::read(bytes, entry).map_err(StageError::Archive)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        file.write_all(&content)?;
        file.sync_all()?;
    }
    Ok(())
}
