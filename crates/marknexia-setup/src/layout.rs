//! Where the preview is installed. Every path comes from the user's own
//! folders; nothing needs elevation.
//!
//! ```text
//! %LOCALAPPDATA%\Programs\Marknexia Rust Preview\    install folder
//!   marknexia-win32.exe, WebView2Loader.dll, README.txt,
//!   THIRD-PARTY-NOTICES.txt, marknexia-sbom.spdx.json,
//!   marknexia-rust-preview-setup.exe                 (uninstaller)
//! %LOCALAPPDATA%\Programs\Marknexia Rust Preview.staging-<token>\  during install
//! %LOCALAPPDATA%\Programs\Marknexia Rust Preview.backup-<token>\   during upgrade
//! %APPDATA%\Microsoft\Windows\Start Menu\Programs\Marknexia (Rust preview).lnk
//! %LOCALAPPDATA%\Marknexia\Updates\                   portable updater (shared root)
//! %LOCALAPPDATA%\Marknexia\settings.json              shared with .NET; never written
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

use crate::identity::{INSTALL_FOLDER_NAME, SETUP_FILE_NAME, SHELL_FILE_NAME, SHORTCUT_FILE_NAME};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutError {
    NotAbsolute(PathBuf),
    /// A path that the registry command line cannot quote.
    Unquotable(PathBuf),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute(path) => write!(
                formatter,
                "the folder {} is not an absolute path",
                path.display()
            ),
            Self::Unquotable(path) => write!(
                formatter,
                "the folder {} contains a quotation mark",
                path.display()
            ),
        }
    }
}

impl std::error::Error for LayoutError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallLayout {
    programs: PathBuf,
    install_dir: PathBuf,
    shortcut: PathBuf,
    updates_root: PathBuf,
    settings_file: PathBuf,
}

impl InstallLayout {
    /// The layout under `local_app_data` (`%LOCALAPPDATA%`) and
    /// `start_menu_programs` (`FOLDERID_Programs`).
    ///
    /// # Errors
    ///
    /// Both must be absolute and free of quotation marks.
    pub fn new(local_app_data: &Path, start_menu_programs: &Path) -> Result<Self, LayoutError> {
        for path in [local_app_data, start_menu_programs] {
            if !path.is_absolute() || path.parent().is_none() {
                return Err(LayoutError::NotAbsolute(path.to_path_buf()));
            }
            if path.to_string_lossy().contains('"') {
                return Err(LayoutError::Unquotable(path.to_path_buf()));
            }
        }
        let programs = local_app_data.join("Programs");
        Ok(Self {
            install_dir: programs.join(INSTALL_FOLDER_NAME),
            programs,
            shortcut: start_menu_programs.join(SHORTCUT_FILE_NAME),
            updates_root: local_app_data.join("Marknexia").join("Updates"),
            settings_file: local_app_data.join("Marknexia").join("settings.json"),
        })
    }

    /// `%LOCALAPPDATA%\Programs`.
    #[must_use]
    pub fn programs(&self) -> &Path {
        &self.programs
    }

    #[must_use]
    pub fn install_dir(&self) -> &Path {
        &self.install_dir
    }

    #[must_use]
    pub fn shell_exe(&self) -> PathBuf {
        self.install_dir.join(SHELL_FILE_NAME)
    }

    #[must_use]
    pub fn setup_exe(&self) -> PathBuf {
        self.install_dir.join(SETUP_FILE_NAME)
    }

    #[must_use]
    pub fn shortcut(&self) -> &Path {
        &self.shortcut
    }

    /// The portable updater's root (`marknexia_update::layout::Layout`).
    #[must_use]
    pub fn updates_root(&self) -> &Path {
        &self.updates_root
    }

    /// The settings file shared with the .NET edition (read only).
    #[must_use]
    pub fn settings_file(&self) -> &Path {
        &self.settings_file
    }

    #[must_use]
    pub fn staging_dir(&self, token: &str) -> PathBuf {
        self.programs
            .join(format!("{INSTALL_FOLDER_NAME}.staging-{token}"))
    }

    #[must_use]
    pub fn backup_dir(&self, token: &str) -> PathBuf {
        self.programs
            .join(format!("{INSTALL_FOLDER_NAME}.backup-{token}"))
    }

    /// A staging or backup folder that an interrupted setup left behind.
    #[must_use]
    pub fn is_leftover_name(name: &str) -> bool {
        [".staging-", ".backup-"].iter().any(|marker| {
            name.strip_prefix(INSTALL_FOLDER_NAME)
                .and_then(|rest| rest.strip_prefix(marker))
                .is_some_and(|token| {
                    !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
        })
    }
}

/// `"<path>"`. Layout paths never contain a quotation mark.
#[must_use]
pub fn quote(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> InstallLayout {
        InstallLayout::new(
            Path::new(r"C:\Users\u\AppData\Local"),
            Path::new(r"C:\Users\u\AppData\Roaming\Microsoft\Windows\Start Menu\Programs"),
        )
        .expect("layout")
    }

    #[test]
    fn installs_per_user_beside_the_dotnet_edition() {
        let layout = layout();
        assert_eq!(
            layout.install_dir(),
            Path::new(r"C:\Users\u\AppData\Local\Programs\Marknexia Rust Preview")
        );
        assert_ne!(
            layout.install_dir(),
            Path::new(r"C:\Users\u\AppData\Local\Programs\Marknexia")
        );
        assert_eq!(
            layout.shell_exe(),
            Path::new(
                r"C:\Users\u\AppData\Local\Programs\Marknexia Rust Preview\marknexia-win32.exe"
            )
        );
        assert_eq!(
            layout
                .setup_exe()
                .file_name()
                .and_then(|name| name.to_str()),
            Some("marknexia-rust-preview-setup.exe")
        );
        assert_eq!(
            layout.shortcut(),
            Path::new(
                r"C:\Users\u\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Marknexia (Rust preview).lnk"
            )
        );
    }

    #[test]
    fn shares_the_updater_root_and_the_settings_file_with_dotnet() {
        let layout = layout();
        assert_eq!(
            layout.updates_root(),
            Path::new(r"C:\Users\u\AppData\Local\Marknexia\Updates")
        );
        assert_eq!(
            layout.settings_file(),
            Path::new(r"C:\Users\u\AppData\Local\Marknexia\settings.json")
        );
    }

    #[test]
    fn staging_and_backup_folders_are_siblings() {
        let layout = layout();
        assert_eq!(layout.staging_dir("ab12").parent(), Some(layout.programs()));
        assert_eq!(layout.backup_dir("ab12").parent(), Some(layout.programs()));
        assert!(InstallLayout::is_leftover_name(
            "Marknexia Rust Preview.staging-ab12"
        ));
        assert!(InstallLayout::is_leftover_name(
            "Marknexia Rust Preview.backup-00ff"
        ));
        for name in [
            "Marknexia Rust Preview",
            "Marknexia Rust Preview.staging-",
            "Marknexia Rust Preview.staging-a b",
            "Marknexia.staging-ab12",
            "Marknexia.backup-ab12",
            "Other.staging-ab12",
        ] {
            assert!(!InstallLayout::is_leftover_name(name), "{name}");
        }
    }

    #[test]
    fn refuses_relative_root_and_quoted_folders() {
        let programs = Path::new(r"C:\Start\Programs");
        assert!(InstallLayout::new(Path::new(r"relative\local"), programs).is_err());
        assert!(InstallLayout::new(Path::new(r"C:\"), programs).is_err());
        assert!(InstallLayout::new(Path::new("C:\\Users\\a\"b"), programs).is_err());
        assert!(InstallLayout::new(Path::new(r"C:\Users\u"), Path::new("Programs")).is_err());
    }

    #[test]
    fn quotes_paths() {
        assert_eq!(quote(Path::new(r"C:\a b\c.exe")), "\"C:\\a b\\c.exe\"");
    }
}
