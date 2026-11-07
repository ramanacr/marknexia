//! Modal dialogs: the folder picker (.NET `FolderPicker`), the About and
//! Help dialogs (.NET `ContentDialog` with a Close button), and the
//! troubleshooting and software components dialogs (.NET `ContentDialog`
//! with extra buttons). Every caller holds no `AppState` borrow: each
//! dialog runs a nested modal loop.
//!
//! The troubleshooting and software components dialogs are system dialog
//! boxes built from an in-memory template: native keyboard handling (Tab,
//! Enter, Esc), UI Automation names from the control texts, system colors
//! (high contrast included), and buttons at least 44 DIPs high.

use std::{ffi::c_void, path::PathBuf};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
        UI::{
            HiDpi::GetDpiForWindow,
            Shell::{
                FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog,
                IFileOpenDialog, SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::{
                DLGTEMPLATE, DialogBoxIndirectParamW, EndDialog, GetDlgItem, MB_ICONINFORMATION,
                MB_OK, MessageBoxW, SendMessageW, WM_COMMAND, WM_INITDIALOG,
            },
        },
    },
    core::{Error, HSTRING},
};

use crate::chrome_model::{
    ABOUT_TITLE, COPY_DIAGNOSTICS_BUTTON, HELP_TITLE, OPEN_DATA_FOLDER_BUTTON, SBOM_COPY_BUTTON,
    SBOM_INTRO, SBOM_LIST_NAME, SBOM_TITLE, TROUBLESHOOTING_DATA_TEXT, TROUBLESHOOTING_HEADING,
    TROUBLESHOOTING_TEXT, TROUBLESHOOTING_TITLE, about_text, help_text,
};

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

/// .NET `CheckForUpdates_Click` "up to date" dialog.
pub(super) fn show_up_to_date(owner: HWND, text: &str) {
    message(owner, marknexia_update::check::UP_TO_DATE_TITLE, text);
}

/// .NET `CheckForUpdates_Click` "update available" dialog for an
/// installable portable update: "Download and restart" and "Later" (the
/// manifest has no release page, so there is no "Open release page").
/// `Some(true)` when the user chose to install; `None` if the dialog could
/// not be shown.
pub(super) fn show_update_available(owner: HWND, text: &str) -> Option<bool> {
    use marknexia_update::check::{AVAILABLE_TITLE, DOWNLOAD_AND_RESTART, LATER};
    let mut template = Template::new(AVAILABLE_TITLE, 260, 84);
    template.item(STATIC, 7, 7, 246, 40, 0xFFFF, CLASS_STATIC, text);
    template.item(
        BUTTON,
        86,
        52,
        92,
        BUTTON_HEIGHT,
        ID_PRIMARY,
        CLASS_BUTTON,
        DOWNLOAD_AND_RESTART,
    );
    template.item(
        DEFAULT_BUTTON,
        183,
        52,
        70,
        BUTTON_HEIGHT,
        ID_CLOSE,
        CLASS_BUTTON,
        LATER,
    );
    template.run(owner, None).map(|result| result == 1)
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

/// Control ids in the template dialogs. The AutomationId of a dialog
/// control is its id.
pub(super) const ID_PRIMARY: u16 = 1;
pub(super) const ID_CLOSE: u16 = 2;
pub(super) const ID_SECONDARY: u16 = 100;
const ID_COMPONENTS: u16 = 101;

/// What the troubleshooting dialog's user chose (.NET
/// `ContentDialogResult`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Choice {
    Primary,
    Secondary,
    Close,
}

/// .NET `ShowSelfHelp_Click` dialog: "Copy diagnostics", "Open data
/// folder", "Close". `None` if the dialog could not be shown.
pub(super) fn show_troubleshooting(owner: HWND) -> Option<Choice> {
    let mut template = Template::new(TROUBLESHOOTING_TITLE, 300, 140);
    template.item(
        STATIC,
        7,
        7,
        286,
        10,
        0xFFFF,
        CLASS_STATIC,
        TROUBLESHOOTING_HEADING,
    );
    template.item(
        STATIC,
        7,
        20,
        286,
        50,
        0xFFFF,
        CLASS_STATIC,
        TROUBLESHOOTING_TEXT,
    );
    template.item(
        STATIC,
        7,
        74,
        286,
        26,
        0xFFFF,
        CLASS_STATIC,
        TROUBLESHOOTING_DATA_TEXT,
    );
    template.item(
        BUTTON,
        7,
        108,
        92,
        BUTTON_HEIGHT,
        ID_PRIMARY,
        CLASS_BUTTON,
        COPY_DIAGNOSTICS_BUTTON,
    );
    template.item(
        BUTTON,
        104,
        108,
        92,
        BUTTON_HEIGHT,
        ID_SECONDARY,
        CLASS_BUTTON,
        OPEN_DATA_FOLDER_BUTTON,
    );
    template.item(
        DEFAULT_BUTTON,
        201,
        108,
        92,
        BUTTON_HEIGHT,
        ID_CLOSE,
        CLASS_BUTTON,
        "Close",
    );
    match template.run(owner, None) {
        Some(1) => Some(Choice::Primary),
        Some(100) => Some(Choice::Secondary),
        Some(_) => Some(Choice::Close),
        None => None,
    }
}

/// .NET `ShowSbom_Click` dialog: the component list, "Copy SBOM" and
/// "Close". `Some(true)` when "Copy SBOM" was chosen; `None` if the dialog
/// could not be shown.
pub(super) fn show_sbom(owner: HWND, components: &[String]) -> Option<bool> {
    let mut template = Template::new(SBOM_TITLE, 300, 236);
    template.item(STATIC, 7, 7, 286, 18, 0xFFFF, CLASS_STATIC, SBOM_INTRO);
    // The list takes its accessible name from this label just before it
    // (.NET `AutomationProperties.Name` of the list).
    template.item(STATIC, 7, 29, 286, 9, 0xFFFF, CLASS_STATIC, SBOM_LIST_NAME);
    template.item(LIST, 7, 40, 286, 160, ID_COMPONENTS, CLASS_LISTBOX, "");
    template.item(
        BUTTON,
        126,
        204,
        80,
        BUTTON_HEIGHT,
        ID_PRIMARY,
        CLASS_BUTTON,
        SBOM_COPY_BUTTON,
    );
    template.item(
        DEFAULT_BUTTON,
        213,
        204,
        80,
        BUTTON_HEIGHT,
        ID_CLOSE,
        CLASS_BUTTON,
        "Close",
    );
    template
        .run(owner, Some(components))
        .map(|result| result == 1)
}

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_BORDER: u32 = 0x0080_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_POPUP: u32 = 0x8000_0000;
const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const DS_SETFONT: u32 = 0x40;
const DS_MODALFRAME: u32 = 0x80;
const DS_CENTER: u32 = 0x800;
const SS_NOPREFIX: u32 = 0x80;
const BS_DEFPUSHBUTTON: u32 = 0x1;
const LBS_NOINTEGRALHEIGHT: u32 = 0x100;
const LB_ADDSTRING: u32 = 0x0180;
const LB_SETITEMHEIGHT: u32 = 0x01A0;
const LB_SETCURSEL: u32 = 0x0186;
const STATIC: u32 = WS_CHILD | WS_VISIBLE | SS_NOPREFIX;
const BUTTON: u32 = WS_CHILD | WS_VISIBLE | WS_TABSTOP;
const DEFAULT_BUTTON: u32 = BUTTON | BS_DEFPUSHBUTTON;
const LIST: u32 =
    WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | WS_VSCROLL | LBS_NOINTEGRALHEIGHT;
const CLASS_BUTTON: u16 = 0x0080;
const CLASS_STATIC: u16 = 0x0082;
const CLASS_LISTBOX: u16 = 0x0083;
/// Dialog units: with Segoe UI 9 pt a vertical unit is at least 1.875
/// pixels at 96 DPI, so 25 units are at least 46 pixels (the 44-DIP
/// target), and they scale with the dialog font at other DPIs.
const BUTTON_HEIGHT: i16 = 25;

/// An in-memory `DLGTEMPLATE` with its items (`DLGITEMTEMPLATE`).
struct Template {
    words: Vec<u16>,
    count: u16,
}

impl Template {
    fn new(title: &str, width: i16, height: i16) -> Self {
        let mut words = Vec::new();
        push_u32(
            &mut words,
            WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME | DS_SETFONT | DS_CENTER,
        );
        push_u32(&mut words, 0);
        // `cdit`, patched by `item`.
        words.push(0);
        for value in [0, 0, width, height] {
            words.push(value as u16);
        }
        // No menu, the default class, then the title.
        words.extend([0, 0]);
        push_text(&mut words, title);
        // DS_SETFONT: point size and typeface.
        words.push(9);
        push_text(&mut words, "Segoe UI");
        Self { words, count: 0 }
    }

    #[allow(clippy::too_many_arguments)]
    fn item(
        &mut self,
        style: u32,
        x: i16,
        y: i16,
        width: i16,
        height: i16,
        id: u16,
        class: u16,
        text: &str,
    ) {
        // Each item starts on a DWORD boundary.
        if self.words.len() % 2 == 1 {
            self.words.push(0);
        }
        push_u32(&mut self.words, style);
        push_u32(&mut self.words, 0);
        for value in [x, y, width, height] {
            self.words.push(value as u16);
        }
        self.words.push(id);
        self.words.extend([0xFFFF, class]);
        push_text(&mut self.words, text);
        // No creation data.
        self.words.push(0);
        self.count += 1;
        self.words[4] = self.count;
    }

    /// Runs the dialog modally; the `EndDialog` value, or `None` when it
    /// could not be created.
    fn run(self, owner: HWND, list: Option<&[String]>) -> Option<isize> {
        // The template must be DWORD-aligned: copy it into u32 storage.
        let mut storage = vec![0_u32; self.words.len().div_ceil(2)];
        // SAFETY: `storage` holds at least as many bytes as `words`.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.words.as_ptr(),
                storage.as_mut_ptr().cast::<u16>(),
                self.words.len(),
            );
        }
        let list = list.map(<[String]>::to_vec);
        let parameter = list
            .as_ref()
            .map_or(0, |list| std::ptr::from_ref(list) as isize);
        // SAFETY: the template and the list outlive the modal call; the
        // dialog procedure only reads the list during WM_INITDIALOG.
        let result = unsafe {
            DialogBoxIndirectParamW(
                None,
                storage.as_ptr().cast::<DLGTEMPLATE>(),
                Some(owner),
                Some(dialog_proc),
                LPARAM(parameter),
            )
        };
        (result > 0).then_some(result)
    }
}

fn push_u32(words: &mut Vec<u16>, value: u32) {
    words.push((value & 0xFFFF) as u16);
    words.push((value >> 16) as u16);
}

fn push_text(words: &mut Vec<u16>, text: &str) {
    words.extend(text.encode_utf16());
    words.push(0);
}

unsafe extern "system" fn dialog_proc(
    dialog: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match message {
        WM_INITDIALOG => {
            // SAFETY: lParam is the list passed by `Template::run`, alive for
            // the modal call, or zero.
            if let Some(list) = unsafe { (lparam.0 as *const Vec<String>).as_ref() }
                && let Ok(control) = unsafe { GetDlgItem(Some(dialog), i32::from(ID_COMPONENTS)) }
            {
                // SAFETY: plain query for this dialog.
                let dpi = unsafe { GetDpiForWindow(dialog) }.max(96);
                let height = crate::layout::Dip::INTERACTION_TARGET.to_pixels(dpi);
                for entry in list {
                    let text: Vec<u16> = entry.encode_utf16().chain(std::iter::once(0)).collect();
                    // SAFETY: the buffer outlives the synchronous call.
                    unsafe {
                        SendMessageW(
                            control,
                            LB_ADDSTRING,
                            Some(WPARAM(0)),
                            Some(LPARAM(text.as_ptr() as isize)),
                        )
                    };
                }
                // SAFETY: plain messages to this dialog's list box.
                unsafe {
                    SendMessageW(
                        control,
                        LB_SETITEMHEIGHT,
                        Some(WPARAM(0)),
                        Some(LPARAM(height as isize)),
                    );
                    SendMessageW(control, LB_SETCURSEL, Some(WPARAM(0)), Some(LPARAM(0)));
                }
            }
            // Focus goes to the first tab stop.
            1
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as u16;
            let code = (wparam.0 >> 16) & 0xFFFF;
            if code == 0 && matches!(id, ID_PRIMARY | ID_CLOSE | ID_SECONDARY) {
                // SAFETY: ends this modal dialog.
                let _ = unsafe { EndDialog(dialog, id as isize) };
                return 1;
            }
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_counts_and_aligns_its_items() {
        let mut template = Template::new("T", 100, 50);
        template.item(STATIC, 1, 2, 3, 4, 0xFFFF, CLASS_STATIC, "ab");
        template.item(
            BUTTON,
            1,
            2,
            3,
            BUTTON_HEIGHT,
            ID_CLOSE,
            CLASS_BUTTON,
            "Close",
        );
        assert_eq!(template.words[4], 2);
        // Header: style, exstyle, cdit, 4 coordinates, menu, class,
        // "T\0", point size, "Segoe UI\0" = 2+2+1+4+2+2+1+9 = 23 words, so
        // the first item is padded to word 24.
        assert_eq!(template.words[23], 0);
        assert_eq!(
            u32::from(template.words[24]) | (u32::from(template.words[25]) << 16),
            STATIC
        );
    }
}
