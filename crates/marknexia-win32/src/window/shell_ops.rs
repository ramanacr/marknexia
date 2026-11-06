//! Clipboard and File Explorer hand-offs for the chrome (CHROME-2 and
//! CHROME-3), and the facts in the troubleshooting copy.
//!
//! The clipboard is written on the UI thread with no shell state borrowed.
//! The Explorer hand-offs run on worker threads with their own COM
//! apartment: a path on an offline share can block, and the window must
//! not wait for it. Every path passes the path gate first.

use std::path::{Path, PathBuf};

use marknexia_files::path::GatedPath;
use windows::{
    Win32::{
        Foundation::{GlobalFree, HANDLE, HWND},
        System::{
            Com::{
                COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
            },
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::CF_UNICODETEXT,
            Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW},
        },
        UI::{
            Shell::{
                ILCreateFromPathW, ILFree, SEE_MASK_CLASSNAME, SEE_MASK_FLAG_NO_UI,
                SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, SHOpenFolderAndSelectItems, ShellExecuteExW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

/// Puts `text` on the clipboard as Unicode text, and nothing else.
///
/// # Errors
///
/// The clipboard is held by another program, or memory ran out.
pub(super) fn copy_text(owner: HWND, text: &str) -> windows::core::Result<()> {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = units.len() * std::mem::size_of::<u16>();
    // Another program may hold the clipboard for a moment.
    let mut opened = Err(windows::core::Error::from_thread());
    for _ in 0..5 {
        // SAFETY: plain clipboard call for this thread's window.
        opened = unsafe { OpenClipboard(Some(owner)) };
        if opened.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    opened?;
    // SAFETY: the clipboard is open on this thread until CloseClipboard;
    // the global block is filled while locked and owned by the clipboard
    // once SetClipboardData succeeds (freed here otherwise).
    let result = unsafe {
        (|| {
            EmptyClipboard()?;
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
            let target = GlobalLock(memory).cast::<u16>();
            if target.is_null() {
                let _ = GlobalFree(Some(memory));
                return Err(windows::core::Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
            let _ = GlobalUnlock(memory);
            if let Err(error) =
                SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.0)))
            {
                let _ = GlobalFree(Some(memory));
                return Err(error);
            }
            Ok(())
        })()
    };
    // SAFETY: closes the clipboard opened above.
    let _ = unsafe { CloseClipboard() };
    result
}

/// A gated Explorer target: the path passed `GatedPath`, and for a
/// repository row it lies inside the gated repository root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RevealTarget(PathBuf);

impl RevealTarget {
    /// `None` when `path` fails the gate, lies outside `root`, or no
    /// repository is open.
    pub(super) fn new(path: &Path, root: Option<&Path>) -> Option<Self> {
        crate::chrome_model::reveal_allowed(path, root).then(|| Self(path.to_path_buf()))
    }
}

/// Opens a File Explorer window with `target` selected in its folder
/// (.NET `RevealFile_Click`). Runs on a worker thread; returns the error
/// message for the diagnostics bar.
pub(super) fn reveal(target: &RevealTarget) -> Result<(), String> {
    with_apartment(|| {
        let path = HSTRING::from(target.0.as_os_str());
        // SAFETY: the HSTRING outlives the call; the item list is freed
        // below.
        let item = unsafe { ILCreateFromPathW(&path) };
        if item.is_null() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound).to_string());
        }
        // SAFETY: a valid absolute item list; no child items, so the item
        // itself is selected in its parent folder.
        let result = unsafe { SHOpenFolderAndSelectItems(item, None, 0) };
        // SAFETY: frees the list created above.
        unsafe { ILFree(Some(item)) };
        result.map_err(|error| error.message())
    })
}

/// .NET "Open data folder": creates `%LOCALAPPDATA%\Marknexia` if needed
/// and opens it in File Explorer through the Folder class, so the folder
/// name never selects another handler. Runs on a worker thread.
pub(super) fn open_folder(folder: &Path) -> Result<(), String> {
    if GatedPath::new(&folder.to_string_lossy()).is_err() {
        return Err("The folder path is not supported.".to_owned());
    }
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    with_apartment(|| {
        let file = HSTRING::from(folder.as_os_str());
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_CLASSNAME | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("open"),
            lpFile: PCWSTR(file.as_ptr()),
            lpClass: w!("Folder"),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // SAFETY: every string outlives the call.
        unsafe { ShellExecuteExW(&mut info) }.map_err(|error| error.message())
    })
}

/// Runs `work` inside a single-threaded apartment on this worker thread.
fn with_apartment(work: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    // SAFETY: balanced with CoUninitialize below on success.
    let entered =
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if entered.is_err() {
        return Err(entered.message());
    }
    let result = work();
    // SAFETY: leaves the apartment entered above.
    unsafe { CoUninitialize() };
    result
}

/// Why the software components dialog cannot open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SbomFailure {
    /// .NET `!File.Exists(path)`.
    Missing,
    /// Unreadable, too large, or not an SPDX package list.
    Failed,
}

/// Reads and parses the SBOM beside the executable (.NET `ShowSbom_Click`).
/// Runs on a worker thread. The handle is opened with identification-only
/// QoS, after the path gate.
pub(super) fn read_sbom(path: &Path) -> Result<(String, Vec<String>), SbomFailure> {
    use std::{io::Read, os::windows::fs::OpenOptionsExt};
    /// `SECURITY_IDENTIFICATION` (std adds `SECURITY_SQOS_PRESENT`).
    const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;
    if GatedPath::new(&path.to_string_lossy()).is_err() {
        return Err(SbomFailure::Failed);
    }
    if !crate::documents::probe_is_file(path) {
        return Err(SbomFailure::Missing);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(path)
        .map_err(|_| SbomFailure::Failed)?;
    let limit = crate::chrome_model::SBOM_MAX_BYTES;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SbomFailure::Failed)?;
    if bytes.len() as u64 > limit {
        return Err(SbomFailure::Failed);
    }
    let json = String::from_utf8(bytes).map_err(|_| SbomFailure::Failed)?;
    // .NET `File.ReadAllTextAsync` drops a UTF-8 byte order mark.
    let json = match json.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_owned(),
        None => json,
    };
    let components =
        crate::chrome_model::sbom_components(&json).map_err(|_| SbomFailure::Failed)?;
    Ok((json, components))
}

/// Native tests isolate the SBOM like the settings file: only an absolute
/// path that passes the path gate is honored.
const SBOM_PATH_VARIABLE: &str = "MARKNEXIA_SBOM_PATH";

/// The SBOM the Help item reads: `MARKNEXIA_SBOM_PATH` when it is an
/// absolute drive or UNC path that passes `GatedPath` (set only by the
/// native tests), otherwise `marknexia-sbom.spdx.json` beside the
/// executable, as .NET reads it.
pub(super) fn sbom_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SBOM_PATH_VARIABLE)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .filter(|path| GatedPath::new(&path.to_string_lossy()).is_ok())
    {
        return Some(path);
    }
    app_folder().map(|folder| folder.join(crate::chrome_model::SBOM_FILE_NAME))
}

/// `%LOCALAPPDATA%\Marknexia`, the .NET `dataDirectory`.
pub(super) fn data_folder() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join("Marknexia"))
}

/// The executable's folder (.NET `AppContext.BaseDirectory`).
pub(super) fn app_folder() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// .NET `Environment.OSVersion.VersionString` on Windows, from the
/// registry (the version functions follow the manifest instead).
pub(super) fn os_version() -> String {
    let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let dword = |name: PCWSTR| {
        let mut value = 0_u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: the buffer is a live local of the given size.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key,
                name,
                RRF_RT_REG_DWORD,
                None,
                Some(std::ptr::from_mut(&mut value).cast()),
                Some(&mut size),
            )
        };
        status.is_ok().then_some(value)
    };
    let mut build = [0_u16; 32];
    let mut size = std::mem::size_of_val(&build) as u32;
    // SAFETY: as above.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(build.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    let build = if status.is_ok() {
        let length = build.iter().position(|unit| *unit == 0).unwrap_or(0);
        String::from_utf16_lossy(&build[..length])
    } else {
        "0".to_owned()
    };
    format!(
        "Microsoft Windows NT {}.{}.{build}.0",
        dword(w!("CurrentMajorVersionNumber")).unwrap_or(10),
        dword(w!("CurrentMinorVersionNumber")).unwrap_or(0),
    )
}

/// The path text with a trailing separator, as .NET reports
/// `AppContext.BaseDirectory`.
pub(super) fn with_trailing_separator(path: &Path) -> String {
    let mut text = path.to_string_lossy().into_owned();
    if !text.ends_with('\\') {
        text.push('\\');
    }
    text
}
