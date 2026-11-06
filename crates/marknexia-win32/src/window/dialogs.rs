//! Modal dialogs: the folder picker (.NET `FolderPicker`) and the About and
//! Help dialogs (.NET `ContentDialog` with a Close button). Every caller
//! holds no `AppState` borrow: each dialog runs a nested modal loop.

use std::{ffi::c_void, path::PathBuf};

use windows::{
    Win32::{
        Foundation::HWND,
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
        UI::{
            Shell::{
                FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog,
                IFileOpenDialog, SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW},
        },
    },
    core::{Error, HSTRING},
};

use crate::chrome_model::{ABOUT_TITLE, HELP_TITLE, about_text, help_text};

/// The common item dialog in folder mode. Cancel returns `None`.
pub(super) fn pick_folder(owner: HWND) -> Result<Option<PathBuf>, Error> {
    const CANCELLED: i32 = 0x8007_04C7_u32 as i32;
    // SAFETY: runs on the shell's UI thread inside the STA the shell keeps
    // for its lifetime (without it, CoCreateInstance fails and the error is
    // reported). Interfaces are released on drop and the display name is
    // freed with CoTaskMemFree.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let options = dialog.GetOptions()?;
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
        if let Err(error) = dialog.Show(Some(owner)) {
            return if error.code().0 == CANCELLED {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let text = name.to_string();
        CoTaskMemFree(Some(name.0 as *const c_void));
        Ok(text.ok().map(PathBuf::from))
    }
}

pub(super) fn show_help(owner: HWND) {
    message(owner, HELP_TITLE, &help_text());
}

pub(super) fn show_about(owner: HWND) {
    message(owner, ABOUT_TITLE, &about_text(env!("CARGO_PKG_VERSION")));
}

/// A system message box: native keyboard, UIA and high-contrast support.
fn message(owner: HWND, title: &str, text: &str) {
    // SAFETY: the HSTRINGs outlive the modal call; `owner` is the live shell
    // window on this thread.
    let _ = unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            &HSTRING::from(title),
            MB_OK | MB_ICONINFORMATION,
        )
    };
}
