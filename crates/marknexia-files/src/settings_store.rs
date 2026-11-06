//! The settings file: load, corrupt-file recovery, legacy backup and
//! atomic save.
//!
//! The document format and validation live in
//! `marknexia_core::settings`. This module owns the file:
//!
//! * Location: `%LOCALAPPDATA%\Marknexia\settings.json`, as in .NET
//!   ([`default_settings_path`]).
//! * Missing file: defaults; the first save creates the file.
//! * Corrupt file: moved aside to `settings.json.corrupt-<id>`; defaults are
//!   used and [`SettingsNotice::Corrupt`] tells the caller. If the move
//!   fails, the store becomes read-only so the file is not overwritten.
//! * Unreadable file (other than not found): defaults, read-only.
//! * Schema version 1 (.NET): read normally. Before the first write, the
//!   current file is copied to `settings.json.v1.bak`.
//! * Newer schema version: values read leniently, read-only.
//! * Every write is atomic ([`write_atomically`]).
//!
//! As in .NET, a save updates the in-memory settings even when the write
//! fails. Unlike .NET, the error is returned instead of swallowed.
//!
//! The store does not coordinate between processes. Two editions that run
//! side by side can still overwrite each other's last change, as two .NET
//! instances can today.

use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use marknexia_core::settings::{
    AppSettings, CURRENT_SCHEMA_VERSION, MAX_SETTINGS_BYTES, ParsedSettings, SettingsParseError,
    parse_settings, serialize_settings,
};

use crate::atomic_write::{unique_suffix, write_atomically};
use crate::bounded_read::{BoundedReadError, BoundedReader};

pub const SETTINGS_DIRECTORY_NAME: &str = "Marknexia";
pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// `%LOCALAPPDATA%\Marknexia\settings.json`, the .NET location
/// (`Environment.SpecialFolder.LocalApplicationData`). `None` when
/// `LOCALAPPDATA` is not set.
///
/// A packaged (MSIX) .NET install writes through file-system virtualization
/// to the package's `LocalCache\Local` folder instead. Finding that copy is
/// a packaging concern for a later phase.
#[must_use]
pub fn default_settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    if base.is_empty() {
        return None;
    }
    Some(
        PathBuf::from(base)
            .join(SETTINGS_DIRECTORY_NAME)
            .join(SETTINGS_FILE_NAME),
    )
}

/// Something about the settings file that the user should be told.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsNotice {
    /// The file could not be used. It was moved to `preserved_as`, and the
    /// defaults are in use.
    Corrupt {
        reason: String,
        preserved_as: PathBuf,
    },
    /// The file could not be used and could not be moved aside. Defaults are
    /// in use and the store is read-only.
    CorruptNotPreserved { reason: String, error: String },
    /// The file exists but could not be read. Defaults are in use and the
    /// store is read-only.
    Unreadable { error: String },
    /// A newer build wrote the file. Known values were read; the store is
    /// read-only so the newer file is not downgraded.
    NewerSchema { found: u32 },
}

impl fmt::Display for SettingsNotice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Corrupt {
                reason,
                preserved_as,
            } => write!(
                formatter,
                "Settings were reset to defaults ({reason}). The previous file was kept as {}.",
                preserved_as.display()
            ),
            Self::CorruptNotPreserved { reason, error } => write!(
                formatter,
                "Settings could not be used ({reason}) or moved aside ({error}). Defaults are in use and changes will not be saved."
            ),
            Self::Unreadable { error } => write!(
                formatter,
                "Settings could not be read ({error}). Defaults are in use and changes will not be saved."
            ),
            Self::NewerSchema { found } => write!(
                formatter,
                "Settings were written by a newer version (schema {found}). Changes will not be saved."
            ),
        }
    }
}

/// Why the store refuses to write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadOnlyReason {
    Unreadable,
    CorruptNotPreserved,
    NewerSchema { found: u32 },
}

#[derive(Debug)]
pub enum SettingsSaveError {
    ReadOnly(ReadOnlyReason),
    /// The legacy backup failed; the settings file was not changed.
    Backup(io::Error),
    Write(io::Error),
}

impl fmt::Display for SettingsSaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly(reason) => write!(formatter, "settings are read-only: {reason:?}"),
            Self::Backup(error) => write!(formatter, "settings backup failed: {error}"),
            Self::Write(error) => write!(formatter, "settings write failed: {error}"),
        }
    }
}

impl std::error::Error for SettingsSaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadOnly(_) => None,
            Self::Backup(error) | Self::Write(error) => Some(error),
        }
    }
}

/// The settings of one process, backed by one file.
#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
    current: AppSettings,
    read_only: Option<ReadOnlyReason>,
    backup_pending: bool,
}

enum ReadOutcome {
    Missing,
    Unreadable(io::Error),
    Bytes(Result<ParsedSettings, SettingsParseError>),
}

impl SettingsStore {
    /// Opens the settings file at `path` and loads it.
    pub fn open(path: impl Into<PathBuf>) -> (Self, Option<SettingsNotice>) {
        let mut store = Self {
            path: path.into(),
            current: AppSettings::default(),
            read_only: None,
            backup_pending: false,
        };
        let notice = store.reload();
        (store, notice)
    }

    /// Opens [`default_settings_path`].
    #[must_use]
    pub fn open_default() -> Option<(Self, Option<SettingsNotice>)> {
        default_settings_path().map(Self::open)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `settings.json.v1.bak` next to the settings file.
    #[must_use]
    pub fn legacy_backup_path(&self) -> PathBuf {
        sibling(&self.path, ".v1.bak")
    }

    #[must_use]
    pub fn current(&self) -> &AppSettings {
        &self.current
    }

    #[must_use]
    pub fn read_only_reason(&self) -> Option<&ReadOnlyReason> {
        self.read_only.as_ref()
    }

    /// Loads the file again (.NET `Load`) and returns a notice when the user
    /// should be told something.
    pub fn reload(&mut self) -> Option<SettingsNotice> {
        self.current = AppSettings::default();
        self.read_only = None;
        self.backup_pending = false;
        match read_settings(&self.path) {
            ReadOutcome::Missing => None,
            ReadOutcome::Unreadable(error) => {
                self.read_only = Some(ReadOnlyReason::Unreadable);
                Some(SettingsNotice::Unreadable {
                    error: error.to_string(),
                })
            }
            ReadOutcome::Bytes(Ok(parsed)) => {
                self.current = parsed.settings;
                if parsed.schema_version > CURRENT_SCHEMA_VERSION {
                    let found = parsed.schema_version;
                    self.read_only = Some(ReadOnlyReason::NewerSchema { found });
                    return Some(SettingsNotice::NewerSchema { found });
                }
                self.backup_pending = parsed.schema_version < CURRENT_SCHEMA_VERSION;
                None
            }
            ReadOutcome::Bytes(Err(error)) => Some(self.preserve_corrupt(&error)),
        }
    }

    /// Normalizes and stores `settings` (.NET `Save`).
    ///
    /// # Errors
    ///
    /// Returns [`SettingsSaveError`] when the store is read-only, when the
    /// legacy backup fails, or when the atomic write fails. The in-memory
    /// settings are updated in every case.
    pub fn save(&mut self, settings: AppSettings) -> Result<(), SettingsSaveError> {
        self.current = settings.normalized();
        if let Some(reason) = &self.read_only {
            return Err(SettingsSaveError::ReadOnly(reason.clone()));
        }
        if self.backup_pending {
            self.back_up_legacy_file()?;
            self.backup_pending = false;
        }
        write_atomically(&self.path, &serialize_settings(&self.current))
            .map_err(SettingsSaveError::Write)
    }

    /// Changes the current settings and saves them.
    ///
    /// # Errors
    ///
    /// As [`Self::save`].
    pub fn update(
        &mut self,
        change: impl FnOnce(&mut AppSettings),
    ) -> Result<(), SettingsSaveError> {
        let mut settings = self.current.clone();
        change(&mut settings);
        self.save(settings)
    }

    /// .NET `AddRecentFile`: an empty or white-space path changes nothing
    /// and is not saved.
    ///
    /// # Errors
    ///
    /// As [`Self::save`].
    pub fn add_recent_file(&mut self, path: &str) -> Result<(), SettingsSaveError> {
        let mut settings = self.current.clone();
        if !settings.add_recent_file(path) {
            return Ok(());
        }
        self.save(settings)
    }

    /// .NET `AddRecentFolder`.
    ///
    /// # Errors
    ///
    /// As [`Self::save`].
    pub fn add_recent_folder(&mut self, path: &str) -> Result<(), SettingsSaveError> {
        let mut settings = self.current.clone();
        if !settings.add_recent_folder(path) {
            return Ok(());
        }
        self.save(settings)
    }

    fn preserve_corrupt(&mut self, error: &SettingsParseError) -> SettingsNotice {
        let reason = error.to_string();
        match move_aside(&self.path) {
            Ok(preserved_as) => SettingsNotice::Corrupt {
                reason,
                preserved_as,
            },
            Err(move_error) => {
                self.read_only = Some(ReadOnlyReason::CorruptNotPreserved);
                SettingsNotice::CorruptNotPreserved {
                    reason,
                    error: move_error.to_string(),
                }
            }
        }
    }

    /// Copies the file on disk to the legacy backup, unless it is gone or
    /// already uses the current schema. A file that became newer since the
    /// load makes the store read-only.
    fn back_up_legacy_file(&mut self) -> Result<(), SettingsSaveError> {
        let bytes = match read_bytes(&self.path) {
            Ok(None) => return Ok(()),
            Ok(Some(Ok(bytes))) => bytes,
            Ok(Some(Err(too_large))) => return Err(SettingsSaveError::Backup(too_large)),
            Err(error) => return Err(SettingsSaveError::Backup(error)),
        };
        if let Ok(parsed) = parse_settings(&bytes) {
            if parsed.schema_version > CURRENT_SCHEMA_VERSION {
                let reason = ReadOnlyReason::NewerSchema {
                    found: parsed.schema_version,
                };
                self.read_only = Some(reason.clone());
                return Err(SettingsSaveError::ReadOnly(reason));
            }
            if parsed.schema_version == CURRENT_SCHEMA_VERSION {
                return Ok(());
            }
        }
        write_atomically(&self.legacy_backup_path(), &bytes).map_err(SettingsSaveError::Backup)
    }
}

fn read_settings(path: &Path) -> ReadOutcome {
    match read_bytes(path) {
        Ok(None) => ReadOutcome::Missing,
        Ok(Some(Ok(bytes))) => ReadOutcome::Bytes(parse_settings(&bytes)),
        Ok(Some(Err(_))) => ReadOutcome::Bytes(Err(SettingsParseError::TooLarge {
            bytes: fs::metadata(path)
                .ok()
                .and_then(|metadata| usize::try_from(metadata.len()).ok())
                .unwrap_or(usize::MAX),
        })),
        Err(error) => ReadOutcome::Unreadable(error),
    }
}

/// `Ok(None)`: not found. `Ok(Some(Err))`: larger than the limit.
fn read_bytes(path: &Path) -> io::Result<Option<io::Result<Vec<u8>>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("settings path is not a file"));
    }
    match BoundedReader::new(MAX_SETTINGS_BYTES).read_from(file) {
        Ok(bytes) => Ok(Some(Ok(bytes))),
        Err(BoundedReadError::TooLarge) => Ok(Some(Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "settings file is too large",
        )))),
        Err(BoundedReadError::Io) => Err(io::Error::other("settings file read failed")),
    }
}

/// Renames `path` to an unused `<name>.corrupt-<id>` sibling.
fn move_aside(path: &Path) -> io::Result<PathBuf> {
    for _ in 0..16 {
        let candidate = sibling(path, &format!(".corrupt-{}", unique_suffix()));
        if fs::symlink_metadata(&candidate).is_ok() {
            continue;
        }
        fs::rename(path, &candidate)?;
        return Ok(candidate);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused name for the corrupt settings file",
    ))
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}
