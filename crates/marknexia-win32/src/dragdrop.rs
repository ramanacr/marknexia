//! Drag and drop of files and folders: port of `MainWindow.Root_DragOver`
//! and `Root_Drop`.
//!
//! The shell window accepts `CF_HDROP` (`DragAcceptFiles` and
//! `WM_DROPFILES`), which is what .NET's `StandardDataFormats.StorageItems`
//! accepts from Explorer, with the copy effect. As in .NET only the first
//! dropped item counts:
//! - a supported Markdown file opens in a tab;
//! - a folder becomes the repository root, and its `README.md` opens when it
//!   exists (`OpenRepositoryFolderAsync`);
//! - any other file shows "Marknexia opens Markdown files (.md, .markdown,
//!   .mdown, .mkdn).".
//!
//! Every dropped path first passes the document path gate
//! ([`crate::documents::check_document_path`]); a refused path opens
//! nothing. Whether the item is a file or a folder is decided off the UI
//! thread, because a UNC path can block until the network times out.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use marknexia_files::repository::is_supported_markdown_file;

use crate::documents::check_document_path;

/// .NET `Root_Drop` diagnostic for a file that is not Markdown.
pub const NOT_MARKDOWN: &str = "Marknexia opens Markdown files (.md, .markdown, .mdown, .mkdn).";
/// Shown when the path gate refuses a dropped path (no .NET equivalent).
pub const REFUSED: &str = "Marknexia cannot open items from this location.";
/// Drops being classified at once; more are ignored until one finishes.
pub const MAX_IN_FLIGHT: usize = 4;

/// What a dropped item is, as seen by the classification thread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemKind {
    File,
    Folder,
    /// The item vanished or cannot be queried; it is treated as a file, so
    /// the open reports why it failed.
    Unknown,
}

/// The shell action for one classified drop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DropAction {
    OpenDocument(PathBuf),
    /// The folder becomes the repository root; `readme` opens when present.
    OpenRepository {
        root: PathBuf,
        readme: Option<PathBuf>,
    },
    NotMarkdown,
}

/// The gate for a dropped path, applied before anything touches the file
/// system.
///
/// # Errors
///
/// [`REFUSED`] when [`check_document_path`] refuses the path.
pub fn gate(path: &str) -> Result<PathBuf, &'static str> {
    check_document_path(path).map_err(|_| REFUSED)?;
    Ok(PathBuf::from(path))
}

/// Decides the action for a gated path (`Root_Drop`'s switch).
#[must_use]
pub fn action(path: &Path, kind: ItemKind, readme_exists: bool) -> DropAction {
    match kind {
        ItemKind::Folder => DropAction::OpenRepository {
            root: path.to_path_buf(),
            readme: readme_exists.then(|| path.join("README.md")),
        },
        ItemKind::File | ItemKind::Unknown => {
            if path.to_str().is_some_and(is_supported_markdown_file) {
                DropAction::OpenDocument(path.to_path_buf())
            } else {
                DropAction::NotMarkdown
            }
        }
    }
}

/// Queries the item (off the UI thread) and decides the action.
#[must_use]
pub fn classify(path: &Path) -> DropAction {
    let (kind, readme) = match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            let readme = path.join("README.md");
            let exists = readme
                .to_str()
                .is_some_and(|text| check_document_path(text).is_ok())
                && readme.is_file();
            (ItemKind::Folder, exists)
        }
        Ok(_) => (ItemKind::File, false),
        Err(_) => (ItemKind::Unknown, false),
    };
    action(path, kind, readme)
}

/// Classified drops waiting for the UI thread, and the count in flight.
#[derive(Clone, Default)]
pub struct DropQueue {
    inner: Arc<Mutex<(VecDeque<DropAction>, usize)>>,
}

impl DropQueue {
    /// Reserves a classification slot; false when [`MAX_IN_FLIGHT`] are
    /// already running.
    pub fn reserve(&self) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.1 >= MAX_IN_FLIGHT {
            return false;
        }
        inner.1 += 1;
        true
    }

    /// Completes a reserved slot with its action (or none, when the worker
    /// could not run).
    pub fn complete(&self, action: Option<DropAction>) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.1 = inner.1.saturating_sub(1);
        inner.0.extend(action);
    }

    pub fn drain(&self) -> Vec<DropAction> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.0.drain(..).collect()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)] // HDROP access stays here.
pub mod native {
    use windows::Win32::{
        Foundation::HGLOBAL,
        System::Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        UI::Shell::{DROPFILES, DragFinish, DragQueryFileW, HDROP},
    };

    /// True when `drop` is a global memory block of this process with a
    /// plausible `DROPFILES` header (the file list offset is inside it).
    fn is_drop_block(drop: HDROP) -> bool {
        let block = HGLOBAL(drop.0);
        // SAFETY: GlobalSize and GlobalLock validate the handle and fail
        // for a value that is not a global memory handle of this process.
        unsafe {
            let size = GlobalSize(block);
            if size < std::mem::size_of::<DROPFILES>() || size > 64 * 1024 * 1024 {
                return false;
            }
            let header = GlobalLock(block).cast::<DROPFILES>();
            if header.is_null() {
                return false;
            }
            let offset = (*header).pFiles as usize;
            let _ = GlobalUnlock(block);
            offset >= std::mem::size_of::<DROPFILES>() && offset < size
        }
    }

    /// The first dropped path, at most 32,767 UTF-16 units, and releases the
    /// drop handle.
    ///
    /// # Safety
    ///
    /// `drop` must be the `HDROP` of the `WM_DROPFILES` being handled; it is
    /// released here and must not be used afterwards.
    pub unsafe fn take_first_path(drop: HDROP) -> Option<String> {
        const MAX_UNITS: u32 = 32_767;
        // Any process at the same or a higher integrity level can post
        // WM_DROPFILES with an arbitrary handle. Use it only when it is a
        // global memory block of this process that holds a DROPFILES header
        // whose file list starts inside the block; otherwise ignore it and
        // never release it.
        if !is_drop_block(drop) {
            return None;
        }
        // SAFETY: per the contract, `drop` is live until DragFinish below.
        let path = unsafe {
            let count = DragQueryFileW(drop, u32::MAX, None);
            if count == 0 {
                None
            } else {
                let length = DragQueryFileW(drop, 0, None);
                if length == 0 || length > MAX_UNITS {
                    None
                } else {
                    let mut buffer = vec![0_u16; length as usize + 1];
                    let copied = DragQueryFileW(drop, 0, Some(&mut buffer)) as usize;
                    (copied == length as usize)
                        .then(|| String::from_utf16(&buffer[..copied]).ok())
                        .flatten()
                }
            }
        };
        unsafe { DragFinish(drop) };
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_refuses_device_and_relative_paths() {
        assert!(gate(r"C:\docs\a.md").is_ok());
        assert!(gate(r"\\server\share\docs").is_ok());
        for refused in [
            r"\\.\pipe\x",
            r"\\?\GLOBALROOT\x",
            r"docs\a.md",
            r"C:\docs\..\a.md",
        ] {
            assert_eq!(gate(refused), Err(REFUSED), "{refused}");
        }
    }

    #[test]
    fn drops_follow_root_drop() {
        assert_eq!(
            action(Path::new(r"C:\d\a.MD"), ItemKind::File, false),
            DropAction::OpenDocument(PathBuf::from(r"C:\d\a.MD"))
        );
        for markdown in ["a.markdown", "a.mdown", "a.mkdn"] {
            let path = Path::new(r"C:\d").join(markdown);
            assert_eq!(
                action(&path, ItemKind::Unknown, false),
                DropAction::OpenDocument(path.clone())
            );
        }
        assert_eq!(
            action(Path::new(r"C:\d\a.txt"), ItemKind::File, false),
            DropAction::NotMarkdown
        );
        assert_eq!(
            action(Path::new(r"C:\repo.md"), ItemKind::Folder, true),
            DropAction::OpenRepository {
                root: PathBuf::from(r"C:\repo.md"),
                readme: Some(PathBuf::from(r"C:\repo.md\README.md")),
            }
        );
        assert_eq!(
            action(Path::new(r"C:\repo"), ItemKind::Folder, false),
            DropAction::OpenRepository {
                root: PathBuf::from(r"C:\repo"),
                readme: None,
            }
        );
    }

    #[test]
    fn classification_reads_the_file_system() {
        let root = std::env::temp_dir().join(format!("marknexia-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = std::path::absolute(&root).unwrap();
        assert_eq!(
            classify(&root),
            DropAction::OpenRepository {
                root: root.clone(),
                readme: None,
            }
        );
        std::fs::write(root.join("README.md"), "# r").unwrap();
        std::fs::write(root.join("notes.txt"), "n").unwrap();
        assert_eq!(
            classify(&root),
            DropAction::OpenRepository {
                root: root.clone(),
                readme: Some(root.join("README.md")),
            }
        );
        assert_eq!(classify(&root.join("notes.txt")), DropAction::NotMarkdown);
        assert_eq!(
            classify(&root.join("missing.md")),
            DropAction::OpenDocument(root.join("missing.md"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_queue_bounds_work_in_flight() {
        let queue = DropQueue::default();
        for _ in 0..MAX_IN_FLIGHT {
            assert!(queue.reserve());
        }
        assert!(!queue.reserve());
        queue.complete(Some(DropAction::NotMarkdown));
        queue.complete(None);
        assert!(queue.reserve());
        assert_eq!(queue.drain(), vec![DropAction::NotMarkdown]);
        assert!(queue.drain().is_empty());
    }
}
