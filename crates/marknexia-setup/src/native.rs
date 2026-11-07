//! Native adapters: `HKEY_CURRENT_USER`, the Start menu shortcut, the
//! association refresh, known folders, message boxes and a read-only
//! WebView2 runtime check. Used only by the executable; tests use the
//! fakes in the library.

use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};

use marknexia_setup::install::ShellIntegration;
use marknexia_setup::registry::{RegOp, RegValue, RegistryBackend};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, IPersistFile,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_NONE,
    REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW,
    RegQueryInfoKeyW, RegSetValueExW,
};
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHCNE_ASSOCCHANGED,
    SHCNF_IDLIST, SHChangeNotify, SHGetKnownFolderPath, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON1, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK,
    MB_SETFOREGROUND, MB_YESNO, MESSAGEBOX_STYLE, MessageBoxW,
};
use windows_core::{HSTRING, Interface, PCWSTR};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status.0 as i32))
    }
}

/// Missing keys and values are not errors for deletes.
fn check_delete(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        check(status)
    }
}

/// An open key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from RegCreateKeyExW or RegOpenKeyExW.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open(
    root: HKEY,
    key: &str,
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> io::Result<Option<Key>> {
    let name = wide(key);
    let mut handle = HKEY::default();
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let status = unsafe { RegOpenKeyExW(root, PCWSTR(name.as_ptr()), None, access, &mut handle) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(status)?;
    Ok(Some(Key(handle)))
}

fn read_string(root: HKEY, key: &str, name: Option<&str>) -> io::Result<Option<String>> {
    let key_name = wide(key);
    let value_name = name.map(wide);
    let value_ptr = value_name
        .as_ref()
        .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr()));
    let mut bytes = 0_u32;
    // SAFETY: the names are NUL-terminated; the first call only sizes.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key_name.as_ptr()),
            value_ptr,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(status)?;
    let mut buffer = vec![0_u16; (bytes as usize).div_ceil(2) + 1];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: `buffer` holds `size` bytes.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key_name.as_ptr()),
            value_ptr,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(status)?;
    let length = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    Ok(Some(String::from_utf16_lossy(&buffer[..length])))
}

/// The real `HKEY_CURRENT_USER`. Keys are relative paths such as
/// `Software\Classes\…`.
pub struct HkcuRegistry;

impl RegistryBackend for HkcuRegistry {
    fn apply(&mut self, op: &RegOp) -> io::Result<()> {
        match op {
            RegOp::Set { key, name, value } => {
                let key_name = wide(key);
                let mut handle = HKEY::default();
                // SAFETY: `key_name` is NUL-terminated; `handle` receives the key.
                check(unsafe {
                    RegCreateKeyExW(
                        HKEY_CURRENT_USER,
                        PCWSTR(key_name.as_ptr()),
                        None,
                        PCWSTR::null(),
                        REG_OPTION_NON_VOLATILE,
                        KEY_WRITE,
                        None,
                        &mut handle,
                        None,
                    )
                })?;
                let key = Key(handle);
                let value_name = name.as_deref().map(wide);
                let value_ptr = value_name
                    .as_ref()
                    .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr()));
                let (kind, data): (REG_VALUE_TYPE, Vec<u8>) = match value {
                    RegValue::String(text) => (
                        REG_SZ,
                        wide(text)
                            .iter()
                            .flat_map(|unit| unit.to_le_bytes())
                            .collect(),
                    ),
                    RegValue::Dword(number) => (REG_DWORD, number.to_le_bytes().to_vec()),
                    RegValue::None => (REG_NONE, Vec::new()),
                };
                // SAFETY: `key` is open for writing; `data` matches `kind`.
                check(unsafe {
                    RegSetValueExW(
                        key.0,
                        value_ptr,
                        None,
                        kind,
                        (!data.is_empty()).then_some(data.as_slice()),
                    )
                })
            }
            RegOp::DeleteValue { key, name } => {
                let key_name = wide(key);
                let value_name = wide(name);
                // SAFETY: both names are NUL-terminated.
                check_delete(unsafe {
                    RegDeleteKeyValueW(
                        HKEY_CURRENT_USER,
                        PCWSTR(key_name.as_ptr()),
                        PCWSTR(value_name.as_ptr()),
                    )
                })
            }
            RegOp::DeleteTree { key } => {
                let key_name = wide(key);
                // SAFETY: `key_name` is NUL-terminated and never empty (the
                // plan only names the preview's own keys).
                check_delete(unsafe {
                    RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr()))
                })?;
                // RegDeleteTreeW empties the key; delete the key itself too.
                check_delete(unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr())) })
            }
            RegOp::DeleteKeyIfEmpty { key } => {
                let Some(open_key) = open(HKEY_CURRENT_USER, key, KEY_READ)? else {
                    return Ok(());
                };
                let mut subkeys = 0_u32;
                let mut values = 0_u32;
                // SAFETY: the key is open; only the counts are requested.
                check(unsafe {
                    RegQueryInfoKeyW(
                        open_key.0,
                        None,
                        None,
                        None,
                        Some(&mut subkeys),
                        None,
                        None,
                        Some(&mut values),
                        None,
                        None,
                        None,
                        None,
                    )
                })?;
                drop(open_key);
                if subkeys == 0 && values == 0 {
                    let key_name = wide(key);
                    // SAFETY: `key_name` is NUL-terminated.
                    check_delete(unsafe {
                        RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr()))
                    })?;
                }
                Ok(())
            }
        }
    }

    fn read_string(&self, key: &str, name: Option<&str>) -> io::Result<Option<String>> {
        read_string(HKEY_CURRENT_USER, key, name)
    }

    fn key_exists(&self, key: &str) -> io::Result<bool> {
        Ok(open(HKEY_CURRENT_USER, key, KEY_READ)?.is_some())
    }
}

/// The installed WebView2 Evergreen runtime version, read-only (the shell
/// offers the install when it is missing; the setup never bundles it).
pub fn webview2_runtime_version() -> Option<String> {
    const CLIENT: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    [
        (
            HKEY_LOCAL_MACHINE,
            format!(r"SOFTWARE\WOW6432Node\{CLIENT}"),
        ),
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\{CLIENT}")),
        (HKEY_CURRENT_USER, format!(r"Software\{CLIENT}")),
    ]
    .into_iter()
    .find_map(|(root, key)| {
        read_string(root, &key, Some("pv"))
            .ok()
            .flatten()
            .filter(|version| !version.is_empty() && version != "0.0.0.0")
    })
}

/// Shortcuts through `IShellLinkW`; needs COM on this thread ([`init_com`]).
pub struct NativeShell;

impl ShellIntegration for NativeShell {
    fn create_shortcut(
        &mut self,
        link: &Path,
        target: &Path,
        working_dir: &Path,
        description: &str,
    ) -> io::Result<()> {
        // SAFETY: COM is initialized on this thread; the HSTRINGs outlive
        // every call that reads them.
        unsafe {
            let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            shell_link.SetPath(&HSTRING::from(target.as_os_str()))?;
            shell_link.SetWorkingDirectory(&HSTRING::from(working_dir.as_os_str()))?;
            shell_link.SetDescription(&HSTRING::from(description))?;
            shell_link.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
            let file: IPersistFile = shell_link.cast()?;
            file.Save(&HSTRING::from(link.as_os_str()), true)?;
        }
        Ok(())
    }

    fn associations_changed(&mut self) {
        // SAFETY: no item pointers are passed with SHCNE_ASSOCCHANGED.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }
}

/// Single-threaded COM for the shortcut. Kept for the process lifetime.
pub fn init_com() -> io::Result<()> {
    // SAFETY: called once, on the main thread, before any COM use.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(io::Error::from)
}

fn known_folder(id: &windows::core::GUID) -> io::Result<PathBuf> {
    // SAFETY: the returned buffer is freed with CoTaskMemFree after copying.
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)?;
        let text = path.to_string();
        CoTaskMemFree(Some(path.0 as *const c_void));
        Ok(PathBuf::from(text.map_err(|_| {
            io::Error::other("the folder path is not valid UTF-16")
        })?))
    }
}

/// `%LOCALAPPDATA%` and the user's Start menu Programs folder.
pub fn user_folders() -> io::Result<(PathBuf, PathBuf)> {
    Ok((
        known_folder(&FOLDERID_LocalAppData)?,
        known_folder(&FOLDERID_Programs)?,
    ))
}

#[derive(Clone, Copy)]
pub enum Icon {
    Information,
    Error,
}

pub fn message(title: &str, text: &str, icon: Icon) {
    let style = match icon {
        Icon::Information => MB_ICONINFORMATION,
        Icon::Error => MB_ICONERROR,
    };
    show(title, text, MB_OK | style);
}

/// Yes/No. `default_yes` selects the default button.
pub fn ask(title: &str, text: &str, default_yes: bool) -> bool {
    let default = if default_yes {
        MB_DEFBUTTON1
    } else {
        MB_DEFBUTTON2
    };
    show(title, text, MB_YESNO | MB_ICONQUESTION | default) == IDYES.0
}

fn show(title: &str, text: &str, style: MESSAGEBOX_STYLE) -> i32 {
    // SAFETY: the HSTRINGs outlive the modal call; no owner window.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(title),
            style | MB_SETFOREGROUND,
        )
    }
    .0
}
