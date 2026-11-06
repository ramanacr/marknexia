//! Drag and drop of files and folders: port of `MainWindow.Root_DragOver`
//! and `Root_Drop`.
//!
//! The shell window is an OLE drop target (`RegisterDragDrop`) that accepts
//! only `CF_HDROP` from the drag's data object, which is what .NET's
//! `StandardDataFormats.StorageItems` accepts from Explorer, with the copy
//! effect. `WM_DROPFILES` is not enabled (`DragAcceptFiles` is never
//! called), so a posted message with a forged handle is ignored. As in .NET
//! only the first dropped item counts:
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

use crate::documents::{check_document_path, probe_is_file, probe_metadata};

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
    // Identification-only QoS opens (`probe_metadata`), after the gate.
    let (kind, readme) = match probe_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            let readme = path.join("README.md");
            let exists = readme
                .to_str()
                .is_some_and(|text| check_document_path(text).is_ok())
                && probe_is_file(&readme);
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
#[allow(unsafe_code)] // OLE drop target, data object and storage medium stay here.
#[allow(non_snake_case)] // COM method names follow the IDropTarget ABI.
pub mod native {
    use std::{
        cell::Cell,
        panic::{AssertUnwindSafe, catch_unwind},
    };

    use windows::{
        Win32::{
            Foundation::{E_UNEXPECTED, HWND, POINTL},
            System::{
                Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL},
                Ole::{
                    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget,
                    IDropTarget_Impl, OleInitialize, OleUninitialize, RegisterDragDrop,
                    ReleaseStgMedium, RevokeDragDrop,
                },
                SystemServices::MODIFIERKEYS_FLAGS,
            },
            UI::Shell::{DragQueryFileW, HDROP},
        },
        core::{Error, HRESULT, Ref, Result, implement},
    };

    /// The longest dropped path read, in UTF-16 units.
    const MAX_UNITS: u32 = 32_767;

    fn hdrop_format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    /// True when the data object offers `CF_HDROP` in global memory.
    fn offers_files(data: &IDataObject) -> bool {
        // SAFETY: a plain query on a live data object; S_OK means yes.
        (unsafe { data.QueryGetData(&hdrop_format()) }) == HRESULT(0)
    }

    /// The first path of the data object's `CF_HDROP`, at most 32,767
    /// UTF-16 units. The storage medium is always released.
    fn first_path(data: &IDataObject) -> Option<String> {
        // SAFETY: GetData returns a storage medium owned by this caller; it
        // is released with ReleaseStgMedium on every path below. The global
        // memory belongs to the medium, so it is read with DragQueryFileW
        // and never freed with DragFinish.
        unsafe {
            let mut medium = data.GetData(&hdrop_format()).ok()?;
            let path = if medium.tymed == TYMED_HGLOBAL.0 as u32 {
                let drop = HDROP(medium.u.hGlobal.0);
                let count = DragQueryFileW(drop, u32::MAX, None);
                let length = if count == 0 {
                    0
                } else {
                    DragQueryFileW(drop, 0, None)
                };
                if length == 0 || length > MAX_UNITS {
                    None
                } else {
                    let mut buffer = vec![0_u16; length as usize + 1];
                    let copied = DragQueryFileW(drop, 0, Some(&mut buffer)) as usize;
                    (copied == length as usize)
                        .then(|| String::from_utf16(&buffer[..copied]).ok())
                        .flatten()
                }
            } else {
                None
            };
            ReleaseStgMedium(&mut medium);
            path
        }
    }

    /// Writes the effect: copy when files are offered and the source allows
    /// copying, none otherwise (.NET `Root_DragOver`).
    fn set_effect(effect: *mut DROPEFFECT, accept: bool) {
        if effect.is_null() {
            return;
        }
        // SAFETY: OLE passes a valid in/out DROPEFFECT for this call.
        unsafe {
            let allowed = *effect;
            *effect = if accept && (allowed.0 & DROPEFFECT_COPY.0) != 0 {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
        }
    }

    /// Runs a callback body; a panic is reported as `E_UNEXPECTED` instead of
    /// unwinding across the COM boundary.
    fn guarded(work: impl FnOnce() -> Result<()>) -> Result<()> {
        catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| Err(Error::from(E_UNEXPECTED)))
    }

    /// The shell's drop target. Not agile: it stays on the window's STA.
    #[implement(IDropTarget, Agile = false)]
    struct DropTarget {
        accept: Cell<bool>,
        on_drop: Box<dyn Fn(String)>,
    }

    impl IDropTarget_Impl for DropTarget_Impl {
        fn DragEnter(
            &self,
            data: Ref<IDataObject>,
            _keys: MODIFIERKEYS_FLAGS,
            _point: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> Result<()> {
            guarded(|| {
                let accept = data.ok().is_ok_and(offers_files);
                self.accept.set(accept);
                set_effect(effect, accept);
                Ok(())
            })
        }

        fn DragOver(
            &self,
            _keys: MODIFIERKEYS_FLAGS,
            _point: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> Result<()> {
            guarded(|| {
                set_effect(effect, self.accept.get());
                Ok(())
            })
        }

        fn DragLeave(&self) -> Result<()> {
            guarded(|| {
                self.accept.set(false);
                Ok(())
            })
        }

        fn Drop(
            &self,
            data: Ref<IDataObject>,
            _keys: MODIFIERKEYS_FLAGS,
            _point: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> Result<()> {
            guarded(|| {
                let accept = self.accept.replace(false);
                set_effect(effect, accept);
                if accept
                    && let Ok(data) = data.ok()
                    && let Some(path) = first_path(data)
                {
                    (self.on_drop)(path);
                }
                Ok(())
            })
        }
    }

    /// OLE initialization and the window's drop-target registration.
    /// [`Self::revoke`] (or drop) revokes the target and uninitializes OLE
    /// on the window thread, before the window is destroyed.
    pub struct DropRegistration {
        window: HWND,
        registered: bool,
        ole: bool,
    }

    impl DropRegistration {
        /// Initializes OLE on this (STA) thread and registers a drop target
        /// for `window`. `on_drop` receives the first dropped path. OLE on
        /// a thread that already entered the STA with `CoInitializeEx` (or
        /// enters it later, as `StaApartment` does) is supported: each
        /// successful call is balanced by its own uninitialization.
        ///
        /// # Errors
        ///
        /// OLE initialization or `RegisterDragDrop` failed.
        pub fn register(window: HWND, on_drop: Box<dyn Fn(String)>) -> Result<Self> {
            // SAFETY: the reserved argument is null; this is the window's
            // own thread.
            unsafe { OleInitialize(None) }?;
            let mut registration = Self {
                window,
                registered: false,
                ole: true,
            };
            let target: IDropTarget = DropTarget {
                accept: Cell::new(false),
                on_drop,
            }
            .into();
            // SAFETY: `window` is this thread's live top-level window; OLE
            // keeps its own reference to the target until RevokeDragDrop.
            unsafe { RegisterDragDrop(window, &target) }?;
            registration.registered = true;
            Ok(registration)
        }

        pub fn revoke(&mut self) {
            if std::mem::take(&mut self.registered) {
                // SAFETY: revokes this window's own registration.
                let _ = unsafe { RevokeDragDrop(self.window) };
            }
            if std::mem::take(&mut self.ole) {
                // SAFETY: balances the successful OleInitialize above, on
                // the same thread.
                unsafe { OleUninitialize() };
            }
        }
    }

    impl Drop for DropRegistration {
        fn drop(&mut self) {
            self.revoke();
        }
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
