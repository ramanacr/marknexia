//! Shared support for the native shell tests (`automation_smoke.rs`,
//! `native_accessibility.rs`), included with `#[path]`.
//!
//! - Every launch gets its own absolute temporary WebView2 user-data folder,
//!   so a test never joins the user's real profile, and never joins a
//!   profile whose browser process from an earlier launch is still exiting
//!   (`ERROR_INVALID_STATE`). The folder is removed after the shell and its
//!   WebView2 processes have exited.
//! - Keys are sent only after re-checking, immediately before `SendInput`,
//!   that the launched shell owns the foreground.

#![allow(dead_code)] // Each test file uses a different subset.

use std::{
    ffi::OsStr,
    path::PathBuf,
    process::{Child, Command},
    sync::atomic::{AtomicU32, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
                SendInput, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
            },
            WindowsAndMessaging::{
                BringWindowToTop, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW,
                GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SW_MINIMIZE, SW_RESTORE,
                SetForegroundWindow, ShowWindowAsync, SwitchToThisWindow, WM_CLOSE,
            },
        },
    },
    core::BOOL,
};

pub const SHELL_CLASS: &str = "MarknexiaRustWindow";

/// The launched shell. Drop closes it (WM_CLOSE, then kill), waits for it
/// and its WebView2 processes to release the profile, and removes the
/// profile folder.
pub struct ShellProcess {
    child: Child,
    profile: PathBuf,
}

impl ShellProcess {
    pub fn launch(executable: &OsStr, args: &[&OsStr]) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let profile = std::env::temp_dir().join(format!(
            "marknexia-native-test-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let profile = std::path::absolute(&profile).expect("absolute profile path");
        // create_dir, not create_dir_all: the name is unique, so this fails
        // rather than adopt an existing folder that Drop would then delete.
        std::fs::create_dir(&profile).expect("create test WebView2 profile");
        let child = Command::new(executable)
            .args(args)
            .env("MARKNEXIA_WEBVIEW2_USER_DATA", &profile)
            .spawn()
            .expect("launch shell");
        Self { child, profile }
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for ShellProcess {
    fn drop(&mut self) {
        // On a runner, leave no modifier logically down after a failed or
        // interrupted chord: a stuck Ctrl reaches the runner's console later
        // (Ctrl+Break put a later pwsh step into its debugger, run
        // 42106410645). Key-up events type nothing.
        if on_github_actions() {
            release_modifiers();
        }
        // A graceful close lets WebView2 shut its browser process down.
        if let Some(window) = find_shell_window(self.id()) {
            // SAFETY: posting to a window of another process; no pointers.
            let _ = unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && matches!(self.child.try_wait(), Ok(None)) {
            thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        // WebView2 processes hold the profile until they exit; removal
        // succeeds only after that.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match std::fs::remove_dir_all(&self.profile) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) if Instant::now() >= deadline => {
                    eprintln!(
                        "warning: test profile {} not removed: {error}",
                        self.profile.display()
                    );
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(250)),
            }
        }
    }
}

struct WindowSearch {
    process_id: u32,
    found: Option<HWND>,
}

unsafe extern "system" fn match_shell_window(hwnd: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the `WindowSearch` borrowed by `find_shell_window`
    // for the duration of the synchronous `EnumWindows` call.
    let search = unsafe { &mut *(data.0 as *mut WindowSearch) };
    let mut owner = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
    if owner != search.process_id || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return true.into();
    }
    let mut class = [0_u16; 64];
    let length = unsafe { GetClassNameW(hwnd, &mut class) };
    let length = usize::try_from(length).unwrap_or(0).min(class.len());
    if String::from_utf16_lossy(&class[..length]) == SHELL_CLASS {
        search.found = Some(hwnd);
        return false.into();
    }
    true.into()
}

/// Binds the tested HWND to the launched process. `FindWindowW` alone can
/// keep returning an older instance of the same class.
pub fn find_shell_window(process_id: u32) -> Option<HWND> {
    let mut search = WindowSearch {
        process_id,
        found: None,
    };
    // EnumWindows reports an error when the callback stops early; the
    // result is read from `search` instead.
    let _ = unsafe {
        EnumWindows(
            Some(match_shell_window),
            LPARAM(std::ptr::from_mut(&mut search) as isize),
        )
    };
    search.found
}

/// The process that owns the foreground window now.
pub fn foreground_owner() -> u32 {
    let foreground = unsafe { GetForegroundWindow() };
    let mut owner = 0;
    unsafe { GetWindowThreadProcessId(foreground, Some(&mut owner)) };
    owner
}

/// Presses the keys in order, then releases them in reverse order. Only for
/// the CI-only Alt unlock; every other key goes through [`send_chord_to`].
pub fn send_chord(keys: &[VIRTUAL_KEY]) {
    let key = |vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    let mut inputs: Vec<INPUT> = keys
        .iter()
        .map(|vk| key(*vk, KEYBD_EVENT_FLAGS(0)))
        .collect();
    inputs.extend(keys.iter().rev().map(|vk| key(*vk, KEYEVENTF_KEYUP)));
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    assert_eq!(sent as usize, inputs.len(), "SendInput");
}

/// Sends the chord only if `process_id` owns the foreground at this instant
/// (re-checked immediately before `SendInput`), so a lost activation can
/// never type into another application.
pub fn send_chord_to(process_id: u32, keys: &[VIRTUAL_KEY]) {
    assert_eq!(
        foreground_owner(),
        process_id,
        "shell must be foreground before SendInput; foreground is {}",
        foreground_description()
    );
    send_chord(keys);
}

/// Sends key-up events for Ctrl, Alt and Shift.
pub fn release_modifiers() {
    let inputs: Vec<INPUT> = [VK_CONTROL, VK_MENU, VK_SHIFT]
        .iter()
        .map(|vk| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: *vk,
                    dwFlags: KEYEVENTF_KEYUP,
                    ..Default::default()
                },
            },
        })
        .collect();
    let _ = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}

/// The foreground window's class, title and owning process, for failure
/// messages when activation is refused.
pub fn foreground_description() -> String {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        return "no window".to_owned();
    }
    let mut class = [0u16; 256];
    let class_len = unsafe { GetClassNameW(foreground, &mut class) }.max(0) as usize;
    let mut title = [0u16; 256];
    let title_len = unsafe { GetWindowTextW(foreground, &mut title) }.max(0) as usize;
    format!(
        "class {:?}, title {:?}, process {}",
        String::from_utf16_lossy(&class[..class_len]),
        String::from_utf16_lossy(&title[..title_len]),
        foreground_owner()
    )
}

/// Whether this run is on a GitHub Actions runner (`CI` and `GITHUB_ACTIONS`
/// both `true`), the only place the synthetic Alt unlock is used.
pub fn on_github_actions() -> bool {
    let set = |name: &str| std::env::var(name).is_ok_and(|v| v.eq_ignore_ascii_case("true"));
    set("CI") && set("GITHUB_ACTIONS")
}

/// One activation attempt for the launched shell window. Windows refuses
/// `SetForegroundWindow` from a background process, so this tries, in order:
/// 1. on GitHub Actions only, a synthetic Alt press, which releases the
///    foreground lock;
/// 2. attaching this thread's input to the foreground thread (when one
///    exists), then `BringWindowToTop` and `SetForegroundWindow`;
/// 3. `SwitchToThisWindow`, which the shell's task switching also uses;
/// 4. on every third attempt, an asynchronous minimize and restore.
///
/// No system setting (such as `SPI_SETFOREGROUNDLOCKTIMEOUT`) is changed.
pub fn activate_attempt(hwnd: HWND, attempt: u32) {
    if on_github_actions() {
        send_chord(&[VK_MENU]);
    }
    let current = unsafe { GetCurrentThreadId() };
    let foreground = unsafe { GetForegroundWindow() };
    let foreground_thread = if foreground.is_invalid() {
        0
    } else {
        unsafe { GetWindowThreadProcessId(foreground, None) }
    };
    let attached = foreground_thread != 0
        && foreground_thread != current
        && unsafe { AttachThreadInput(current, foreground_thread, true) }.as_bool();
    let _ = unsafe { BringWindowToTop(hwnd) };
    let _ = unsafe { SetForegroundWindow(hwnd) };
    if attached {
        let _ = unsafe { AttachThreadInput(current, foreground_thread, false) };
    }
    unsafe { SwitchToThisWindow(hwnd, true) };
    if attempt % 3 == 2 {
        // Asynchronous: a busy shell cannot block this test thread.
        unsafe {
            let _ = ShowWindowAsync(hwnd, SW_MINIMIZE);
            let _ = ShowWindowAsync(hwnd, SW_RESTORE);
        }
        let _ = unsafe { SetForegroundWindow(hwnd) };
    }
}
