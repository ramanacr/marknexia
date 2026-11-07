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
//! - The settings file (`MARKNEXIA_SETTINGS_PATH`) lives in the same
//!   temporary folder, so a test never reads or writes the user's settings.

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
        Foundation::{CloseHandle, HWND, LPARAM, WPARAM},
        System::Threading::{
            AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
                SendInput, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_MENU, VK_SHIFT,
            },
            WindowsAndMessaging::{
                BringWindowToTop, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW,
                GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SW_MINIMIZE, SW_RESTORE,
                SetForegroundWindow, ShowWindowAsync, SwitchToThisWindow, WM_CLOSE,
            },
        },
    },
    core::{BOOL, PWSTR},
};

pub const SHELL_CLASS: &str = "MarknexiaRustWindow";

/// Two small Markdown documents in a fresh temporary folder, removed on
/// drop. Tests that need two tabs open them: without documents the shell
/// starts with one start-page tab ("Welcome"; CHROME-27).
pub struct TwoDocuments {
    folder: PathBuf,
    paths: [PathBuf; 2],
}

impl TwoDocuments {
    pub fn new() -> Self {
        let folder = std::env::temp_dir().join(format!("mnx-docs-{}", unique_suffix()));
        std::fs::create_dir(&folder).expect("document folder");
        let folder = std::path::absolute(&folder).expect("absolute document folder");
        let paths = [folder.join("first.md"), folder.join("second.md")];
        std::fs::write(&paths[0], "# First\n\nThe first document.\n").expect("first.md");
        std::fs::write(&paths[1], "# Second\n\nThe second document.\n").expect("second.md");
        Self { folder, paths }
    }

    /// The command-line arguments that open both documents.
    pub fn args(&self) -> [&OsStr; 2] {
        [self.paths[0].as_os_str(), self.paths[1].as_os_str()]
    }
}

impl Drop for TwoDocuments {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// The launched shell. Drop closes it (WM_CLOSE, then kill), waits for it
/// and its WebView2 processes to release the profile, and removes the
/// profile folder.
pub struct ShellProcess {
    child: Child,
    profile: PathBuf,
}

/// A launch-unique suffix: this process, a counter and the time.
fn unique_suffix() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!(
        "{}-{}-{nanos}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A single-instance key no other launch uses (`MARKNEXIA_INSTANCE_KEY`
/// accepts at most 64 characters).
pub fn unique_instance_key() -> String {
    let mut key = format!("mnx-test-{}", unique_suffix());
    key.truncate(64);
    key
}

impl ShellProcess {
    /// Launches an isolated shell: its own WebView2 profile and its own
    /// single-instance key, so it never hands off to another running shell.
    pub fn launch(executable: &OsStr, args: &[&OsStr]) -> Self {
        Self::launch_with_options(executable, args, None, &unique_instance_key())
    }

    /// Like [`Self::launch`], with `settings` written to the isolated
    /// settings file before the shell starts.
    pub fn launch_with_settings(
        executable: &OsStr,
        args: &[&OsStr],
        settings: Option<&[u8]>,
    ) -> Self {
        Self::launch_with_options(executable, args, settings, &unique_instance_key())
    }

    /// Launches a shell that owns, or hands off to, `instance_key`.
    pub fn launch_with_key(executable: &OsStr, args: &[&OsStr], instance_key: &str) -> Self {
        Self::launch_with_options(executable, args, None, instance_key)
    }

    /// Like [`Self::launch`], with `browser_folder` passed to WebView2 as
    /// `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` (relative to the isolated
    /// profile). A folder that does not exist makes the loader report no
    /// runtime, without touching the installed one.
    pub fn launch_with_browser_folder(
        executable: &OsStr,
        args: &[&OsStr],
        browser_folder: &str,
    ) -> Self {
        Self::launch_inner(
            executable,
            args,
            None,
            &unique_instance_key(),
            Some(browser_folder),
        )
    }

    fn launch_with_options(
        executable: &OsStr,
        args: &[&OsStr],
        settings: Option<&[u8]>,
        instance_key: &str,
    ) -> Self {
        Self::launch_inner(executable, args, settings, instance_key, None)
    }

    fn launch_inner(
        executable: &OsStr,
        args: &[&OsStr],
        settings: Option<&[u8]>,
        instance_key: &str,
        browser_folder: Option<&str>,
    ) -> Self {
        let profile =
            std::env::temp_dir().join(format!("marknexia-native-test-{}", unique_suffix()));
        let profile = std::path::absolute(&profile).expect("absolute profile path");
        // create_dir, not create_dir_all: the name is unique, so this fails
        // rather than adopt an existing folder that Drop would then delete.
        std::fs::create_dir(&profile).expect("create test WebView2 profile");
        let settings_path = profile.join("settings.json");
        if let Some(settings) = settings {
            std::fs::write(&settings_path, settings).expect("write test settings");
        }
        let mut command = Command::new(executable);
        command
            .args(args)
            .env("MARKNEXIA_WEBVIEW2_USER_DATA", &profile)
            .env("MARKNEXIA_SETTINGS_PATH", &settings_path)
            // The Help > Software components item reads this isolated file
            // (absent unless a test writes it), never the executable folder.
            .env("MARKNEXIA_SBOM_PATH", profile.join("sbom.spdx.json"))
            // Each Reveal in File Explorer hand-off is recorded here.
            .env("MARKNEXIA_REVEAL_LOG", profile.join("reveal.log"))
            // Each followed link's timeline is recorded here.
            .env("MARKNEXIA_LINK_TIMING_LOG", profile.join("link-timing.log"))
            .env("MARKNEXIA_INSTANCE_KEY", instance_key);
        match browser_folder {
            Some(folder) => {
                command.env("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", profile.join(folder));
            }
            // Never inherit an override from the test runner.
            None => {
                command.env_remove("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER");
            }
        }
        let child = command.spawn().expect("launch shell");
        Self { child, profile }
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// The isolated settings file the shell reads and writes.
    pub fn settings_path(&self) -> PathBuf {
        self.profile.join("settings.json")
    }

    /// The isolated SBOM path (`MARKNEXIA_SBOM_PATH`); absent until a test
    /// writes it.
    pub fn sbom_path(&self) -> PathBuf {
        self.profile.join("sbom.spdx.json")
    }
    /// The File Explorer hand-off record (`MARKNEXIA_REVEAL_LOG`).
    pub fn reveal_log_path(&self) -> PathBuf {
        self.profile.join("reveal.log")
    }
    /// The link navigation timelines (`MARKNEXIA_LINK_TIMING_LOG`).
    pub fn link_timing_log_path(&self) -> PathBuf {
        self.profile.join("link-timing.log")
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

/// GitHub Actions only: the hosted windows-11-arm image raises a "Microsoft
/// account" sign-in prompt (a system `Windows.UI.Core.CoreWindow`) that holds
/// the foreground (runs 42104089745, 42110166762). The ephemeral runner never
/// signs in, so the prompt is asked to close; nothing is typed into it.
fn close_runner_sign_in_prompt() {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        return;
    }
    let mut class = [0u16; 256];
    let class_len = unsafe { GetClassNameW(foreground, &mut class) }.max(0) as usize;
    let mut title = [0u16; 256];
    let title_len = unsafe { GetWindowTextW(foreground, &mut title) }.max(0) as usize;
    if String::from_utf16_lossy(&class[..class_len]) != "Windows.UI.Core.CoreWindow" {
        return;
    }
    let title = String::from_utf16_lossy(&title[..title_len]);
    let image = process_image(foreground_owner()).to_ascii_lowercase();
    // System shell surfaces only: the sign-in prompt (WWAHost) and system
    // flyouts such as Search (SystemApps), which took the foreground after
    // the prompt closed in run 42113919802.
    if title == "Microsoft account"
        || image.ends_with("\\wwahost.exe")
        || image.contains("\\windows\\systemapps\\")
    {
        eprintln!(
            "dismissing a runner system surface ({})",
            foreground_description()
        );
        // SAFETY: posting to a window of another process; no pointers.
        let _ = unsafe { PostMessageW(Some(foreground), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        // Flyouts close on Escape; the foreground is this system surface,
        // never an application.
        send_chord(&[VK_ESCAPE]);
    }
}

/// The executable path of a process, when it can be queried.
fn process_image(process_id: u32) -> String {
    let Ok(process) =
        (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) })
    else {
        return "unknown".to_owned();
    };
    let mut path = [0u16; 1024];
    let mut length = path.len() as u32;
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    match queried {
        Ok(()) => String::from_utf16_lossy(&path[..length as usize]),
        Err(_) => "unknown".to_owned(),
    }
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
    let owner = foreground_owner();
    format!(
        "class {:?}, title {:?}, process {} ({})",
        String::from_utf16_lossy(&class[..class_len]),
        String::from_utf16_lossy(&title[..title_len]),
        owner,
        process_image(owner)
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
        close_runner_sign_in_prompt();
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
    if on_github_actions() {
        leave_menu_mode(hwnd);
    }
}

/// The synthetic Alt tap can reach the shell after it became the
/// foreground window. A lone Alt opens the shell's menu bar (keyboard menu
/// mode), which would then swallow the test's keys (run 42199272748: Ctrl+Tab
/// from the document did not switch tabs). Leave menu mode with Escape,
/// sent only while the shell owns the foreground and is in menu mode.
fn leave_menu_mode(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{GUITHREADINFO, GetGUIThreadInfo};
    const MENU_MODES: u32 = 0x4 | 0x8 | 0x10; // GUI_INMENUMODE | GUI_SYSTEMMENUMODE | GUI_POPUPMENUMODE
    let mut process = 0;
    let ui_thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process)) };
    if ui_thread == 0 {
        return;
    }
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let in_menu = unsafe { GetGUIThreadInfo(ui_thread, &mut info) }.is_ok()
            && info.flags.0 & MENU_MODES != 0;
        if in_menu && foreground_owner() == process {
            eprintln!("leaving the shell's menu mode after the Alt unlock");
            send_chord_to(process, &[VK_ESCAPE]);
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}
