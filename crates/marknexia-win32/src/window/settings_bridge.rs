//! The shell's settings file: loaded once at startup, saved on each change
//! (.NET `SettingsService.Save` after each setter in `MainWindow`).

use std::path::PathBuf;

use marknexia_core::settings::AppSettings;
use marknexia_files::settings_store::{SettingsNotice, SettingsSaveError, SettingsStore};

/// Test harnesses isolate the settings file the same way they isolate the
/// WebView2 profile; only an absolute path is honored.
const SETTINGS_PATH_VARIABLE: &str = "MARKNEXIA_SETTINGS_PATH";

/// Opens the settings file: `MARKNEXIA_SETTINGS_PATH` when it is absolute,
/// otherwise `%LOCALAPPDATA%\Marknexia\settings.json`. `None` when no path
/// is known (no `LOCALAPPDATA`); the shell then runs on defaults.
pub(super) fn open() -> Option<(SettingsStore, Option<SettingsNotice>)> {
    if let Some(path) = std::env::var_os(SETTINGS_PATH_VARIABLE)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(SettingsStore::open(path));
    }
    SettingsStore::open_default()
}

/// The settings in use: the store's, or the defaults without a store.
pub(super) fn current(store: Option<&SettingsStore>) -> AppSettings {
    store
        .map(|store| store.current().clone())
        .unwrap_or_default()
}

/// Applies `change` and saves. A read-only store was already reported by
/// its load notice ("changes will not be saved"), so that error is not
/// repeated; other failures return a message for the diagnostics bar.
pub(super) fn update(
    store: Option<&mut SettingsStore>,
    change: impl FnOnce(&mut AppSettings),
) -> Option<String> {
    let store = store?;
    match store.update(change) {
        Ok(()) | Err(SettingsSaveError::ReadOnly(_)) => None,
        Err(error) => Some(save_failure(&error)),
    }
}

/// .NET `AddRecentFile` and save.
pub(super) fn add_recent_file(store: Option<&mut SettingsStore>, path: &str) -> Option<String> {
    let store = store?;
    match store.add_recent_file(path) {
        Ok(()) | Err(SettingsSaveError::ReadOnly(_)) => None,
        Err(error) => Some(save_failure(&error)),
    }
}

/// .NET `AddRecentFolder` and save.
pub(super) fn add_recent_folder(store: Option<&mut SettingsStore>, path: &str) -> Option<String> {
    let store = store?;
    match store.add_recent_folder(path) {
        Ok(()) | Err(SettingsSaveError::ReadOnly(_)) => None,
        Err(error) => Some(save_failure(&error)),
    }
}

fn save_failure(error: &SettingsSaveError) -> String {
    format!("Settings could not be saved ({error}).")
}
