//! The portable update folder.
//!
//! ```text
//! <root>\                     owner-only DACL (default %LOCALAPPDATA%\Marknexia\Updates)
//!   versions\<version>-<arch>\  verified, side-by-side payloads
//!   staging\pending-<random>\   a payload being extracted and checked
//!   active.json                 the active-version pointer
//!   transaction.json            an activation in progress
//!   health\ack-<pid>.json       a new version's health acknowledgement
//!   last-update-failure.json    the last rollback's diagnostic
//! ```

use std::path::{Path, PathBuf};

use crate::{manifest::Architecture, version::Version};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `%LOCALAPPDATA%\Marknexia\Updates` (the .NET updater's root).
    #[must_use]
    pub fn default_root() -> Option<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA")?;
        let local = PathBuf::from(local);
        local
            .is_absolute()
            .then(|| local.join("Marknexia").join("Updates"))
    }

    /// The layout and payload id of an executable inside
    /// `<root>\versions\<id>\`, or `None` when it is not in one.
    #[must_use]
    pub fn from_payload_executable(executable: &Path) -> Option<(Self, String)> {
        let payload = executable.parent()?;
        let id = payload.file_name()?.to_str()?;
        let versions = payload.parent()?;
        if !versions
            .file_name()?
            .to_str()?
            .eq_ignore_ascii_case("versions")
            || !is_payload_id(id)
        {
            return None;
        }
        Some((Self::new(versions.parent()?.to_path_buf()), id.to_owned()))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    #[must_use]
    pub fn staging(&self) -> PathBuf {
        self.root.join("staging")
    }

    #[must_use]
    pub fn health(&self) -> PathBuf {
        self.root.join("health")
    }

    #[must_use]
    pub fn active_file(&self) -> PathBuf {
        self.root.join("active.json")
    }

    #[must_use]
    pub fn transaction_file(&self) -> PathBuf {
        self.root.join("transaction.json")
    }

    #[must_use]
    pub fn failure_file(&self) -> PathBuf {
        self.root.join("last-update-failure.json")
    }

    /// The highest version seen in a verified manifest.
    #[must_use]
    pub fn highest_seen_file(&self) -> PathBuf {
        self.root.join("highest-seen.json")
    }

    /// `versions\<id>`; `id` must pass [`is_payload_id`].
    #[must_use]
    pub fn payload(&self, id: &str) -> PathBuf {
        self.versions().join(id)
    }
}

/// `<version>-<x64|arm64>`.
#[must_use]
pub fn payload_id(version: &Version, architecture: Architecture) -> String {
    format!("{version}-{}", architecture.asset_token())
}

/// The version text of a payload id (`1.2.0` for `1.2.0-x64`).
#[must_use]
pub fn payload_version(id: &str) -> Option<&str> {
    ["x64", "arm64"].iter().find_map(|token| {
        id.strip_suffix(token)
            .and_then(|rest| rest.strip_suffix('-'))
            .filter(|version| Version::parse(version).is_ok())
    })
}

/// A payload folder name the updater could have created. Only such names
/// are ever followed from a pointer, so a pointer cannot leave `versions`.
#[must_use]
pub fn is_payload_id(id: &str) -> bool {
    ["x64", "arm64"].iter().any(|token| {
        id.strip_suffix(token)
            .and_then(|rest| rest.strip_suffix('-'))
            .is_some_and(|version| Version::parse(version).is_ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_ids_are_versions_with_an_architecture() {
        for id in ["1.2.0-x64", "1.2.0-arm64", "1.2.0-rc.1-x64"] {
            assert!(is_payload_id(id), "{id}");
        }
        for id in [
            "",
            "x64",
            "1.2-x64",
            "1.2.0",
            "1.2.0-x86",
            "..",
            "1.2.0-x64/..",
            "1.2.0-X64",
        ] {
            assert!(!is_payload_id(id), "{id}");
        }
    }

    #[test]
    fn only_executables_in_a_versions_folder_have_a_layout() {
        let root = PathBuf::from(r"C:\Users\u\AppData\Local\Marknexia\Updates");
        let exe = root
            .join("versions")
            .join("1.2.0-x64")
            .join("marknexia-win32.exe");
        assert_eq!(
            Layout::from_payload_executable(&exe),
            Some((Layout::new(root.clone()), "1.2.0-x64".to_owned()))
        );
        let elsewhere = root
            .join("other")
            .join("1.2.0-x64")
            .join("marknexia-win32.exe");
        assert_eq!(Layout::from_payload_executable(&elsewhere), None);
        let bad_id = root
            .join("versions")
            .join("latest")
            .join("marknexia-win32.exe");
        assert_eq!(Layout::from_payload_executable(&bad_id), None);
    }
}
