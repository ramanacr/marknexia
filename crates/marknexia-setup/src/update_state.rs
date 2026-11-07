//! The portable updater's state, as seen by the setup.
//!
//! Install never touches `%LOCALAPPDATA%\Marknexia\Updates`. The installed
//! folder is the "unpacked installation" of `compat/decisions/portable-updater.md`
//! (UPD-11): the updater never changes it, stages newer verified payloads
//! in `Updates\versions\<version>-<arch>` and switches `active.json`. A
//! normal start of the installed shell forwards to an active payload only
//! when it is newer than the installed version (UPD-12), so installing a
//! newer setup over an updated installation makes the installed copy win
//! again, and an older pointer is simply ignored.
//!
//! Uninstall removes only what the Rust updater writes (the
//! `marknexia_update::layout::Layout` names) and never follows a reparse
//! point. The .NET updater's `pending-<guid>` folders and every other entry
//! stay. A payload whose shell still runs blocks the uninstall before
//! anything is removed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use marknexia_update::layout::{Layout, is_payload_id};

use crate::identity::SHELL_FILE_NAME;

/// What uninstall found and removed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateStateReport {
    pub removed: Vec<PathBuf>,
    /// Entries that were kept on purpose (not the Rust updater's, or a
    /// reparse point).
    pub kept: Vec<PathBuf>,
}

/// A reparse point (symbolic link or junction) or a missing path.
pub(crate) fn is_reparse_point(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Ok(true);
        }
    }
    Ok(metadata.file_type().is_symlink())
}

/// The shells of payloads that the updater staged.
///
/// # Errors
/// The `versions` folder could not be listed.
pub fn payload_shells(root: &Path) -> io::Result<Vec<PathBuf>> {
    let layout = Layout::new(root.to_path_buf());
    let versions = layout.versions();
    match is_reparse_point(root).and_then(|root_link| Ok(root_link || is_reparse_point(&versions)?))
    {
        Ok(false) => {}
        Ok(true) => return Ok(Vec::new()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    let mut shells = Vec::new();
    for entry in fs::read_dir(&versions)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str().is_some_and(is_payload_id) && !is_reparse_point(&entry.path())? {
            let shell = entry.path().join(SHELL_FILE_NAME);
            if shell.is_file() {
                shells.push(shell);
            }
        }
    }
    shells.sort();
    Ok(shells)
}

fn remove_file_if_present(path: &Path, report: &mut UpdateStateReport) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {
            report.removed.push(path.to_path_buf());
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_dir_if_empty(path: &Path, report: &mut UpdateStateReport) {
    if fs::remove_dir(path).is_ok() {
        report.removed.push(path.to_path_buf());
    }
}

/// Removes the children of `folder` that `owned` accepts, then `folder`
/// when it is empty. Reparse points are kept and never followed.
fn remove_owned_children(
    folder: &Path,
    owned: impl Fn(&str, bool) -> bool,
    report: &mut UpdateStateReport,
) -> io::Result<()> {
    match is_reparse_point(folder) {
        Ok(false) => {}
        Ok(true) => {
            report.kept.push(folder.to_path_buf());
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if is_reparse_point(&path)? {
            report.kept.push(path);
            continue;
        }
        let is_dir = entry.file_type()?.is_dir();
        if !owned(&name, is_dir) {
            report.kept.push(path);
            continue;
        }
        if is_dir {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
        report.removed.push(path);
    }
    remove_dir_if_empty(folder, report);
    Ok(())
}

/// Removes the Rust updater's state under `root`.
///
/// # Errors
/// A file could not be removed (for example a payload's shell that still
/// runs). Callers check [`payload_shells`] for running shells first.
pub fn remove(root: &Path) -> io::Result<UpdateStateReport> {
    let mut report = UpdateStateReport::default();
    match is_reparse_point(root) {
        Ok(true) => {
            report.kept.push(root.to_path_buf());
            return Ok(report);
        }
        Ok(false) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(error) => return Err(error),
    }
    let layout = Layout::new(root.to_path_buf());
    // The pointer first: a half-removed payload is then never forwarded to.
    for file in [
        layout.active_file(),
        layout.transaction_file(),
        layout.highest_seen_file(),
        layout.failure_file(),
    ] {
        remove_file_if_present(&file, &mut report)?;
    }
    remove_owned_children(
        &layout.versions(),
        |name, is_dir| is_dir && is_payload_id(name),
        &mut report,
    )?;
    remove_owned_children(
        &layout.staging(),
        |name, is_dir| is_dir && name.starts_with("pending-"),
        &mut report,
    )?;
    remove_owned_children(
        &layout.health(),
        |name, is_dir| !is_dir && name.starts_with("ack-") && name.ends_with(".json"),
        &mut report,
    )?;
    remove_dir_if_empty(root, &mut report);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }

    #[test]
    fn a_missing_root_is_nothing_to_do() {
        let dir = TempDir::new("updates-missing");
        let root = dir.path().join("Updates");
        assert_eq!(remove(&root).expect("remove"), UpdateStateReport::default());
        assert!(payload_shells(&root).expect("shells").is_empty());
    }

    #[test]
    fn removes_rust_state_and_keeps_dotnet_and_unknown_entries() {
        let dir = TempDir::new("updates-mixed");
        let root = dir.path().join("Updates");
        // Rust updater state (text stand-ins, no executable bytes).
        write(&root.join("active.json"), "{}");
        write(&root.join("transaction.json"), "{}");
        write(&root.join("highest-seen.json"), "{}");
        write(&root.join("last-update-failure.json"), "{}");
        write(
            &root
                .join("versions")
                .join("0.2.0-x64")
                .join(SHELL_FILE_NAME),
            "stand-in",
        );
        write(
            &root.join("versions").join("0.3.0-arm64").join("README.txt"),
            "r",
        );
        write(&root.join("staging").join("pending-abc").join("x.txt"), "x");
        write(&root.join("health").join("ack-42.json"), "{}");
        // Not the Rust updater's.
        write(
            &root
                .join("pending-0123456789abcdef")
                .join("package.zip.txt"),
            "dotnet",
        );
        write(
            &root.join("versions").join("latest").join("x.txt"),
            "unknown",
        );
        write(&root.join("notes.txt"), "user");

        assert_eq!(
            payload_shells(&root).expect("shells"),
            [root
                .join("versions")
                .join("0.2.0-x64")
                .join(SHELL_FILE_NAME)]
        );
        let report = remove(&root).expect("remove");

        assert!(!root.join("active.json").exists());
        assert!(!root.join("transaction.json").exists());
        assert!(!root.join("highest-seen.json").exists());
        assert!(!root.join("versions").join("0.2.0-x64").exists());
        assert!(!root.join("versions").join("0.3.0-arm64").exists());
        assert!(
            !root.join("staging").exists(),
            "an emptied staging folder is removed"
        );
        assert!(!root.join("health").exists());
        assert!(
            root.join("pending-0123456789abcdef")
                .join("package.zip.txt")
                .is_file()
        );
        assert!(root.join("versions").join("latest").join("x.txt").is_file());
        assert!(root.join("notes.txt").is_file());
        assert!(report.kept.contains(&root.join("versions").join("latest")));
    }

    #[test]
    fn an_emptied_root_is_removed_but_its_parent_is_kept() {
        let dir = TempDir::new("updates-only-rust");
        let parent = dir.path().join("Marknexia");
        let root = parent.join("Updates");
        write(&root.join("active.json"), "{}");
        write(
            &root.join("versions").join("1.0.0-x64").join("README.txt"),
            "r",
        );
        write(&parent.join("settings.json"), "{}");
        remove(&root).expect("remove");
        assert!(!root.exists());
        assert!(
            parent.join("settings.json").is_file(),
            "shared settings stay"
        );
    }

    #[test]
    fn a_locked_payload_file_stops_the_removal() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = TempDir::new("updates-locked");
        let root = dir.path().join("Updates");
        let shell = root
            .join("versions")
            .join("0.2.0-x64")
            .join(SHELL_FILE_NAME);
        write(&shell, "stand-in");
        let _lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&shell)
            .expect("lock");
        assert!(remove(&root).is_err());
    }
}
