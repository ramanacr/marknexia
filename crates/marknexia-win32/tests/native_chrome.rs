#![cfg(windows)]

//! Native gates for the shell chrome (Phase 2 tasks 1, 2 and 6): toolbar,
//! menu bar, sidebar views, repository tree, settings persistence and the
//! diagnostics bar, through the platform UIA proxies of the native controls.
//!
//! Ignored by default: they need `MARKNEXIA_SHELL_EXE` and an unlocked
//! interactive desktop. Every launch uses an isolated settings file and
//! WebView2 profile (`support/shell.rs`). Keys are sent only through
//! `send_chord_to`, which re-checks the foreground owner first.

use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use marknexia_core::settings::parse_settings;
use windows::{
    Win32::{
        Foundation::HWND,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Variant::VARIANT,
        },
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationElement,
                IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
                IUIAutomationSelectionItemPattern, TreeScope_Children, TreeScope_Descendants,
                UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID,
                UIA_ComboBoxControlTypeId, UIA_ControlTypePropertyId, UIA_ExpandCollapsePatternId,
                UIA_InvokePatternId, UIA_ListControlTypeId, UIA_ListItemControlTypeId,
                UIA_MenuBarControlTypeId, UIA_NamePropertyId, UIA_SelectionItemPatternId,
                UIA_TabControlTypeId, UIA_TabItemControlTypeId, UIA_TreeControlTypeId,
                UIA_TreeItemControlTypeId,
            },
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetThreadDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{VK_LEFT, VK_MENU, VK_RETURN},
        },
    },
    core::{BSTR, Interface},
};

#[path = "support/shell.rs"]
mod shell_support;

use shell_support::{
    ShellProcess, activate_attempt, find_shell_window, foreground_owner, send_chord_to,
};

const TIMEOUT: Duration = Duration::from_secs(20);
const TARGET_DIP: f64 = 44.0;

struct Apartment;

impl Apartment {
    fn enter() -> Self {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .expect("UIA STA");
        Self
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/markdown/gfm")
        .join(name)
}

/// A fresh, empty temporary folder, removed on drop.
struct TempFolder(PathBuf);

impl TempFolder {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "marknexia-chrome-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create temporary folder");
        Self(std::path::absolute(path).expect("absolute temporary folder"))
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn json_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

struct Launched {
    // Drop order: UIA objects, then the apartment, then the process.
    root: IUIAutomationElement,
    automation: IUIAutomation,
    _apartment: Apartment,
    hwnd: HWND,
    shell: ShellProcess,
}

fn launch(args: &[&std::ffi::OsStr], settings: Option<&[u8]>) -> Launched {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let shell = ShellProcess::launch_with_settings(&executable, args, settings);
    let hwnd = wait_for("the launched shell HWND", || find_shell_window(shell.id()));
    let apartment = Apartment::enter();
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .expect("CUIAutomation");
    let root = unsafe { automation.ElementFromHandle(hwnd) }.expect("ElementFromHandle");
    Launched {
        root,
        automation,
        _apartment: apartment,
        hwnd,
        shell,
    }
}

fn name(element: &IUIAutomationElement) -> String {
    unsafe { element.CurrentName() }
        .map(|name| name.to_string())
        .unwrap_or_default()
}

impl Launched {
    fn find_all(
        &self,
        scope_root: &IUIAutomationElement,
        kind: UIA_CONTROLTYPE_ID,
        children_only: bool,
    ) -> Vec<IUIAutomationElement> {
        let Ok(condition) = (unsafe {
            self.automation
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(kind.0))
        }) else {
            return Vec::new();
        };
        let scope = if children_only {
            TreeScope_Children
        } else {
            TreeScope_Descendants
        };
        let Ok(found) = (unsafe { scope_root.FindAll(scope, &condition) }) else {
            return Vec::new();
        };
        (0..unsafe { found.Length() }.unwrap_or(0))
            .filter_map(|index| unsafe { found.GetElement(index) }.ok())
            .collect()
    }

    /// The first element of `kind` named `wanted` anywhere in the shell.
    fn find(&self, kind: UIA_CONTROLTYPE_ID, wanted: &str) -> Option<IUIAutomationElement> {
        let Ok(by_name) = (unsafe {
            self.automation
                .CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(wanted)))
        }) else {
            return None;
        };
        let Ok(by_kind) = (unsafe {
            self.automation
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(kind.0))
        }) else {
            return None;
        };
        let condition = unsafe { self.automation.CreateAndCondition(&by_name, &by_kind) }.ok()?;
        unsafe { self.root.FindFirst(TreeScope_Descendants, &condition) }.ok()
    }

    /// The element whose AutomationId is `id` (a Win32 control's ID).
    fn find_id(&self, id: &str) -> Option<IUIAutomationElement> {
        let condition = unsafe {
            self.automation
                .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(BSTR::from(id)))
        }
        .ok()?;
        unsafe { self.root.FindFirst(TreeScope_Descendants, &condition) }.ok()
    }

    /// Waits for the control with AutomationId `id`, then asserts its
    /// control type and accessible name.
    fn wait_id(&self, id: &str, kind: UIA_CONTROLTYPE_ID, wanted: &str) -> IUIAutomationElement {
        let element = self.until(&format!("AutomationId {id} ({wanted:?})"), || {
            self.find_id(id)
                .filter(|element| unsafe { element.CurrentControlType() }.is_ok_and(|k| k == kind))
                .filter(|element| name(element) == wanted)
        });
        assert_eq!(
            name(&element),
            wanted,
            "accessible name of AutomationId {id}"
        );
        element
    }

    /// `wait_for` that prints the shell's UIA subtree when it times out.
    fn until<T>(&self, what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(value) = probe() {
                return value;
            }
            if Instant::now() >= deadline {
                eprintln!("UIA subtree of the shell window (depth <= 4):");
                self.dump(&self.root, 0);
                panic!("timed out waiting for {what}");
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn dump(&self, element: &IUIAutomationElement, depth: usize) {
        let kind = unsafe { element.CurrentControlType() }.map_or(0, |kind| kind.0);
        let id = unsafe { element.CurrentAutomationId() }
            .map(|id| id.to_string())
            .unwrap_or_default();
        let offscreen = unsafe { element.CurrentIsOffscreen() }.is_ok_and(|o| o.as_bool());
        eprintln!(
            "{:indent$}- {:?} type={kind} id={id:?}{}",
            "",
            name(element),
            if offscreen { " offscreen" } else { "" },
            indent = depth * 2
        );
        if depth >= 4 {
            return;
        }
        let Ok(walker) = (unsafe { self.automation.RawViewWalker() }) else {
            return;
        };
        let mut child = unsafe { walker.GetFirstChildElement(element) }.ok();
        while let Some(current) = child {
            self.dump(&current, depth + 1);
            child = unsafe { walker.GetNextSiblingElement(&current) }.ok();
        }
    }

    fn minimum_pixels(&self) -> f64 {
        TARGET_DIP * f64::from(unsafe { GetDpiForWindow(self.hwnd) }) / 96.0
    }

    fn activate(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut attempt = 0;
        while foreground_owner() != self.shell.id() && Instant::now() < deadline {
            activate_attempt(self.hwnd, attempt);
            attempt += 1;
            thread::sleep(Duration::from_millis(200));
        }
        foreground_owner() == self.shell.id()
    }
}

fn is_enabled(element: &IUIAutomationElement) -> bool {
    unsafe { element.CurrentIsEnabled() }.is_ok_and(|enabled| enabled.as_bool())
}

fn is_onscreen(element: &IUIAutomationElement) -> bool {
    unsafe { element.CurrentIsOffscreen() }.is_ok_and(|offscreen| !offscreen.as_bool())
}

fn invoke(element: &IUIAutomationElement) {
    let pattern: IUIAutomationInvokePattern =
        unsafe { element.GetCurrentPattern(UIA_InvokePatternId) }
            .expect("Invoke pattern")
            .cast()
            .expect("Invoke interface");
    unsafe { pattern.Invoke() }.expect("Invoke");
}

fn assert_target(element: &IUIAutomationElement, minimum: f64, check_width: bool) {
    let rect = unsafe { element.CurrentBoundingRectangle() }.expect("BoundingRectangle");
    let width = f64::from(rect.right - rect.left);
    let height = f64::from(rect.bottom - rect.top);
    eprintln!("{:?}: {width} x {height} px", name(element));
    assert!(
        height >= minimum,
        "{:?} is {height} px high; minimum {minimum}",
        name(element)
    );
    if check_width {
        assert!(
            width >= minimum,
            "{:?} is {width} px wide; minimum {minimum}",
            name(element)
        );
    }
}

fn read_settings(shell: &ShellProcess) -> Option<marknexia_core::settings::AppSettings> {
    let bytes = std::fs::read(shell.settings_path()).ok()?;
    parse_settings(&bytes).ok().map(|parsed| parsed.settings)
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn chrome_controls_are_native_named_and_meet_the_44_dip_target() {
    let features = fixture("features.md");
    let shell = launch(&[features.as_os_str()], None);
    let minimum = shell.minimum_pixels();

    for (id, button) in [
        ("2001", "Toggle sidebar"),
        ("2002", "Back"),
        ("2003", "Forward"),
        ("2004", "Reload document"),
        ("2005", "Open repository folder"),
        ("2008", "Find in document"),
    ] {
        let element = shell.wait_id(id, UIA_ButtonControlTypeId, button);
        assert_target(&element, minimum, true);
    }
    let theme = shell.wait_id("2007", UIA_ComboBoxControlTypeId, "Theme");
    assert_target(&theme, minimum, true);

    let selector = shell.wait_id("1003", UIA_TabControlTypeId, "Sidebar view");
    let views: Vec<String> = shell
        .find_all(&selector, UIA_TabItemControlTypeId, true)
        .iter()
        .map(name)
        .collect();
    assert_eq!(views, ["Outline", "Repository", "Recent files"]);
    for item in shell.find_all(&selector, UIA_TabItemControlTypeId, true) {
        assert_target(&item, minimum, false);
    }

    // The outline comes from the rendered document's headings.
    let outline = shell.wait_id("2011", UIA_ListControlTypeId, "Document outline");
    let heading = shell.until("the outline's first heading", || {
        shell
            .find_all(&outline, UIA_ListItemControlTypeId, true)
            .into_iter()
            .find(|item| name(item) == "GFM Full Features Test")
    });
    assert_target(&heading, minimum, false);
    // A newly opened document has no history yet; it can be reloaded.
    assert!(!is_enabled(&shell.wait_id(
        "2002",
        UIA_ButtonControlTypeId,
        "Back"
    )));
    assert!(is_enabled(&shell.wait_id(
        "2004",
        UIA_ButtonControlTypeId,
        "Reload document"
    )));

    let menus: Vec<String> = shell
        .find_all(&shell.root, UIA_MenuBarControlTypeId, false)
        .iter()
        .flat_map(|bar| {
            shell
                .find_all(
                    bar,
                    windows::Win32::UI::Accessibility::UIA_MenuItemControlTypeId,
                    true,
                )
                .into_iter()
                .map(|item| name(&item))
                .collect::<Vec<_>>()
        })
        .collect();
    for menu in ["File", "View", "Settings", "Help"] {
        assert!(
            menus.iter().any(|item| item == menu),
            "menu {menu} in {menus:?}"
        );
    }
    // The opened file joined the recent files in the isolated settings.
    let settings = shell.until("the recent file to be saved", || {
        read_settings(&shell.shell).filter(|settings| !settings.recent_files.is_empty())
    });
    assert!(settings.recent_files[0].ends_with("features.md"));
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn toggling_the_sidebar_hides_it_and_persists_the_setting() {
    let features = fixture("features.md");
    let shell = launch(&[features.as_os_str()], None);
    let outline = shell.wait_id("2011", UIA_ListControlTypeId, "Document outline");
    assert!(is_onscreen(&outline));
    invoke(&shell.wait_id("2001", UIA_ButtonControlTypeId, "Toggle sidebar"));
    shell.until("IsSidebarOpen=false in the settings file", || {
        read_settings(&shell.shell)
            .filter(|settings| !settings.is_sidebar_open)
            .map(drop)
    });
    shell.until("the outline to leave the screen", || {
        (shell
            .find(UIA_ListControlTypeId, "Document outline")
            .is_none()
            || !is_onscreen(&outline))
        .then_some(())
    });
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn the_saved_repository_is_restored_as_a_lazily_filled_tree() {
    let repository = TempFolder::new("repo");
    std::fs::write(repository.0.join("README.md"), "# Repository readme\n").unwrap();
    std::fs::create_dir(repository.0.join("docs")).unwrap();
    std::fs::write(repository.0.join("docs").join("guide.md"), "# Guide\n").unwrap();
    std::fs::write(repository.0.join("notes.bin"), [0_u8; 4]).unwrap();
    let settings = format!(
        "{{\"RepositoryRoot\": {}, \"SidebarMode\": 1}}",
        json_string(&repository.0.to_string_lossy())
    );
    let shell = launch(&[], Some(settings.as_bytes()));

    let tree = shell.wait_id("2012", UIA_TreeControlTypeId, "Repository files");
    let root_name = repository
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let root = shell.until("the repository root item", || {
        shell
            .find_all(&tree, UIA_TreeItemControlTypeId, true)
            .into_iter()
            .find(|item| name(item) == root_name)
    });
    let children = shell.until("the root's children", || {
        let names: Vec<String> = shell
            .find_all(&root, UIA_TreeItemControlTypeId, true)
            .iter()
            .map(name)
            .collect();
        (!names.is_empty()).then_some(names)
    });
    // Folders first, unsupported files left out (.NET RepositoryTreeBuilder).
    assert_eq!(children, ["docs", "README.md"]);
    let docs = shell
        .find_all(&root, UIA_TreeItemControlTypeId, true)
        .into_iter()
        .find(|item| name(item) == "docs")
        .unwrap();
    let expand: IUIAutomationExpandCollapsePattern =
        unsafe { docs.GetCurrentPattern(UIA_ExpandCollapsePatternId) }
            .expect("ExpandCollapse pattern")
            .cast()
            .expect("ExpandCollapse interface");
    unsafe { expand.Expand() }.expect("Expand");
    shell.until("guide.md after expansion", || {
        shell
            .find_all(&docs, UIA_TreeItemControlTypeId, true)
            .into_iter()
            .find(|item| name(item) == "guide.md")
    });
    let selector = shell.wait_id("1003", UIA_TabControlTypeId, "Sidebar view");
    let selected: Vec<String> = shell
        .find_all(&selector, UIA_TabItemControlTypeId, true)
        .into_iter()
        .filter(|item| {
            unsafe { item.GetCurrentPattern(UIA_SelectionItemPatternId) }
                .ok()
                .and_then(|pattern| pattern.cast::<IUIAutomationSelectionItemPattern>().ok())
                .is_some_and(|pattern| {
                    unsafe { pattern.CurrentIsSelected() }.is_ok_and(|selected| selected.as_bool())
                })
        })
        .map(|item| name(&item))
        .collect();
    assert_eq!(selected, ["Repository"]);
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn corrupt_settings_show_a_dismissible_warning() {
    let shell = launch(&[], Some(b"{ this is not JSON"));
    let warning = shell.until("the settings warning", || {
        shell
            .find_id("2020")
            .filter(|text| name(text).starts_with("Warning: Settings were reset to defaults"))
    });
    assert_eq!(
        unsafe { warning.CurrentControlType() }.unwrap(),
        windows::Win32::UI::Accessibility::UIA_TextControlTypeId,
        "the diagnostics message is exposed as text"
    );
    assert!(is_onscreen(&warning));
    let profile = shell.shell.settings_path().parent().unwrap().to_path_buf();
    let preserved = std::fs::read_dir(&profile)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("settings.json.corrupt-")
        });
    assert!(preserved, "the corrupt file is kept beside the new one");
    let close = shell.wait_id("2021", UIA_ButtonControlTypeId, "Close");
    assert_target(&close, shell.minimum_pixels(), true);
    invoke(&close);
    shell.until("the diagnostics bar to close", || {
        (!is_onscreen(&warning) || name(&warning).is_empty()).then_some(())
    });
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn enter_on_a_heading_jumps_and_alt_left_goes_back() {
    let features = fixture("features.md");
    let shell = launch(&[features.as_os_str()], None);
    let outline = shell.wait_id("2011", UIA_ListControlTypeId, "Document outline");
    let items = shell.until("outline items", || {
        let items = shell.find_all(&outline, UIA_ListItemControlTypeId, true);
        (items.len() > 2).then_some(items)
    });
    let back = shell.wait_id("2002", UIA_ButtonControlTypeId, "Back");
    let forward = shell.wait_id("2003", UIA_ButtonControlTypeId, "Forward");
    assert!(!is_enabled(&back));

    let pattern: IUIAutomationSelectionItemPattern =
        unsafe { items[2].GetCurrentPattern(UIA_SelectionItemPatternId) }
            .expect("SelectionItem pattern")
            .cast()
            .expect("SelectionItem interface");
    unsafe { pattern.Select() }.expect("Select");
    assert!(
        shell.activate(),
        "the shell must own the foreground to send keys"
    );
    unsafe { outline.SetFocus() }.expect("focus the outline");
    send_chord_to(shell.shell.id(), &[VK_RETURN]);
    shell.until("Back to be enabled after the jump", || {
        is_enabled(&back).then_some(())
    });
    send_chord_to(shell.shell.id(), &[VK_MENU, VK_LEFT]);
    shell.until("Forward to be enabled after Alt+Left", || {
        is_enabled(&forward).then_some(())
    });
    assert!(!is_enabled(&back));
}

// ---------------------------------------------------------------------------
// CHROME-2 and CHROME-3 ports: name parity, brand, tooltips, compact layout,
// Help items, row buttons and the dark menu bar.
// ---------------------------------------------------------------------------

use windows::Win32::{
    Foundation::{LPARAM, RECT, WPARAM},
    System::{
        DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard},
        Memory::{GlobalLock, GlobalUnlock},
    },
    UI::{
        Accessibility::{
            UIA_ImageControlTypeId, UIA_ProcessIdPropertyId, UIA_TextControlTypeId,
            UIA_WindowControlTypeId,
        },
        WindowsAndMessaging::{
            EnumWindows, GW_OWNER, GetClassNameW, GetWindow, GetWindowTextW, PostMessageW,
            SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SendMessageTimeoutW,
            SetWindowPos, WM_CLOSE, WM_COMMAND,
        },
    },
};

const MENU_TROUBLESHOOTING: usize = 2133;
const MENU_SBOM: usize = 2134;
const TTM_GETTOOLCOUNT: u32 = 0x040D;

impl Launched {
    fn dpi(&self) -> f64 {
        f64::from(unsafe { GetDpiForWindow(self.hwnd) })
    }

    /// A menu command, posted as the menu bar would send it (no input).
    fn post_command(&self, id: usize) {
        unsafe { PostMessageW(Some(self.hwnd), WM_COMMAND, WPARAM(id), LPARAM(0)) }
            .expect("post WM_COMMAND");
    }

    /// The diagnostics message once its accessible name starts with
    /// `prefix`.
    fn wait_diagnostic(&self, prefix: &str) -> String {
        self.until(&format!("the diagnostic {prefix:?}"), || {
            self.find_id("2020")
                .map(|text| name(&text))
                .filter(|text| text.starts_with(prefix))
        })
    }

    /// A top-level window of the shell process named `title` (a dialog).
    fn find_dialog(&self, title: &str) -> Option<IUIAutomationElement> {
        let root = unsafe { self.automation.GetRootElement() }.ok()?;
        let by_name = unsafe {
            self.automation
                .CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(title)))
        }
        .ok()?;
        let by_process = unsafe {
            self.automation.CreatePropertyCondition(
                UIA_ProcessIdPropertyId,
                &VARIANT::from(self.shell.id() as i32),
            )
        }
        .ok()?;
        let condition =
            unsafe { self.automation.CreateAndCondition(&by_name, &by_process) }.ok()?;
        unsafe { root.FindFirst(TreeScope_Children, &condition) }.ok()
    }

    /// The control with AutomationId `id` inside `scope`.
    fn find_id_in(&self, scope: &IUIAutomationElement, id: &str) -> Option<IUIAutomationElement> {
        let condition = unsafe {
            self.automation
                .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(BSTR::from(id)))
        }
        .ok()?;
        unsafe { scope.FindFirst(TreeScope_Descendants, &condition) }.ok()
    }

    fn resize(&self, width_dip: f64, height_dip: f64) {
        let scale = self.dpi() / 96.0;
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                (width_dip * scale) as i32,
                (height_dip * scale) as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
        .expect("resize the shell window");
    }
}

fn width_of(element: &IUIAutomationElement) -> f64 {
    let rect = unsafe { element.CurrentBoundingRectangle() }.expect("BoundingRectangle");
    f64::from(rect.right - rect.left)
}

/// The clipboard's Unicode text, read from this test process.
fn clipboard_text() -> Option<String> {
    let mut opened = false;
    for _ in 0..20 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if !opened {
        return None;
    }
    let text = unsafe {
        GetClipboardData(13).ok().and_then(|handle| {
            let memory = windows::Win32::Foundation::HGLOBAL(handle.0);
            let pointer = GlobalLock(memory).cast::<u16>();
            if pointer.is_null() {
                return None;
            }
            let mut length = 0;
            while *pointer.add(length) != 0 {
                length += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, length));
            let _ = GlobalUnlock(memory);
            Some(text)
        })
    };
    let _ = unsafe { CloseClipboard() };
    text
}

/// Top-level windows of `class` (with their owner, for tooltips).
fn top_level_windows(class: &str) -> Vec<HWND> {
    struct Search {
        class: Vec<u16>,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> windows::core::BOOL {
        let search = unsafe { &mut *(data.0 as *mut Search) };
        let mut name = [0_u16; 64];
        let length = unsafe { GetClassNameW(hwnd, &mut name) };
        if usize::try_from(length).is_ok_and(|length| name[..length] == search.class[..]) {
            search.found.push(hwnd);
        }
        true.into()
    }
    let mut search = Search {
        class: class.encode_utf16().collect(),
        found: Vec::new(),
    };
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut search as *mut Search as isize)) };
    search.found
}

fn window_title(hwnd: HWND) -> String {
    let mut text = [0_u16; 260];
    let length = unsafe { GetWindowTextW(hwnd, &mut text) };
    String::from_utf16_lossy(&text[..usize::try_from(length).unwrap_or(0)])
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn the_brand_tooltips_and_open_button_follow_dotnet() {
    let features = fixture("features.md");
    let shell = launch(&[features.as_os_str()], None);
    let minimum = shell.minimum_pixels();
    // .NET `OpenFileButton` name.
    let open = shell.wait_id("1001", UIA_ButtonControlTypeId, "Open Markdown file");
    assert_target(&open, minimum, true);
    // .NET `BrandIcon` (an image named "Marknexia") and `BrandDetails`.
    let icon = shell.wait_id("2030", UIA_ImageControlTypeId, "Marknexia");
    assert!(is_onscreen(&icon));
    let product = shell.wait_id("2031", UIA_TextControlTypeId, "Marknexia");
    assert!(is_onscreen(&product));
    let tagline = shell.wait_id("2032", UIA_TextControlTypeId, "Read. Explore. Understand.");
    assert!(is_onscreen(&tagline));
    // One tooltip window owned by the shell: six toolbar buttons, the
    // resize handle and the two row buttons.
    let tooltips: Vec<HWND> = top_level_windows("tooltips_class32")
        .into_iter()
        .filter(|tip| unsafe { GetWindow(*tip, GW_OWNER) }.is_ok_and(|owner| owner == shell.hwnd))
        .collect();
    assert_eq!(tooltips.len(), 1, "one tooltip window for the chrome");
    let mut count = 0_usize;
    unsafe {
        SendMessageTimeoutW(
            tooltips[0],
            TTM_GETTOOLCOUNT,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            5000,
            Some(&mut count),
        )
    };
    assert_eq!(count, 9, "tools registered with the tooltip");
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn the_compact_layout_collapses_labels_and_closes_the_pane() {
    let features = fixture("features.md");
    let shell = launch(&[features.as_os_str()], None);
    let minimum = shell.minimum_pixels();
    let scale = shell.dpi() / 96.0;
    shell.resize(1100.0, 760.0);
    let outline = shell.wait_id("2011", UIA_ListControlTypeId, "Document outline");
    shell.until("the outline on screen in the wide layout", || {
        is_onscreen(&outline).then_some(())
    });
    let folder = shell.wait_id("2005", UIA_ButtonControlTypeId, "Open repository folder");
    let theme = shell.wait_id("2007", UIA_ComboBoxControlTypeId, "Theme");
    assert!((width_of(&folder) - 96.0 * scale).abs() <= 2.0);
    assert!((width_of(&theme) - 110.0 * scale).abs() <= 2.0);

    // Below 720 DIPs: .NET `CompactWindow`.
    shell.resize(600.0, 700.0);
    shell.until("the pane to close in the compact layout", || {
        (!is_onscreen(&outline)).then_some(())
    });
    shell.until("the brand details to collapse", || {
        shell
            .find_id("2031")
            .is_none_or(|details| !is_onscreen(&details))
            .then_some(())
    });
    shell.until("the Folder label to collapse", || {
        ((width_of(&folder) - 44.0 * scale).abs() <= 2.0).then_some(())
    });
    assert!(
        (width_of(&theme) - 86.0 * scale).abs() <= 2.0,
        "compact theme width"
    );
    let open = shell.wait_id("1001", UIA_ButtonControlTypeId, "Open Markdown file");
    assert_target(&open, minimum, true);
    // The brand image stays.
    assert!(is_onscreen(&shell.wait_id(
        "2030",
        UIA_ImageControlTypeId,
        "Marknexia"
    )));

    // The toggle opens the 240-DIP pane (and saves IsSidebarOpen).
    invoke(&shell.wait_id("2001", UIA_ButtonControlTypeId, "Toggle sidebar"));
    shell.until("the pane to open in the compact layout", || {
        is_onscreen(&outline).then_some(())
    });
    assert!(
        width_of(&outline) <= 240.0 * scale + 2.0,
        "compact pane length"
    );
    shell.until("IsSidebarOpen=true in the settings file", || {
        read_settings(&shell.shell)
            .filter(|settings| settings.is_sidebar_open)
            .map(drop)
    });
    invoke(&shell.wait_id("2001", UIA_ButtonControlTypeId, "Toggle sidebar"));
    shell.until("the pane to close again", || {
        (!is_onscreen(&outline)).then_some(())
    });

    // Back above 720 DIPs: .NET `WideWindow` opens the pane.
    shell.resize(1100.0, 760.0);
    shell.until("the pane to open in the wide layout", || {
        is_onscreen(&outline).then_some(())
    });
    shell.until("the Folder label to return", || {
        ((width_of(&folder) - 96.0 * scale).abs() <= 2.0).then_some(())
    });
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn the_help_menu_lists_troubleshooting_and_sbom_without_updates() {
    let shell = launch(&[], None);
    let help = shell.until("the Help menu", || {
        shell.find(
            windows::Win32::UI::Accessibility::UIA_MenuItemControlTypeId,
            "Help",
        )
    });
    assert!(
        shell.activate(),
        "the shell must own the foreground to open its menu"
    );
    let expand: IUIAutomationExpandCollapsePattern =
        unsafe { help.GetCurrentPattern(UIA_ExpandCollapsePatternId) }
            .expect("ExpandCollapse pattern")
            .cast()
            .expect("ExpandCollapse interface");
    unsafe { expand.Expand() }.expect("Expand Help");
    let root = unsafe { shell.automation.GetRootElement() }.expect("desktop root");
    let items = shell.until("the Help items", || {
        let names: Vec<String> = shell
            .find_all(
                &root,
                windows::Win32::UI::Accessibility::UIA_MenuItemControlTypeId,
                false,
            )
            .iter()
            .filter(|item| {
                unsafe { item.CurrentProcessId() }.is_ok_and(|id| id as u32 == shell.shell.id())
            })
            .map(name)
            .collect();
        names
            .iter()
            .any(|item| item == "About Marknexia")
            .then_some(names)
    });
    let _ = unsafe { expand.Collapse() };
    for wanted in [
        "Keyboard shortcuts",
        "Troubleshooting and diagnostics",
        "Software components SBOM",
        "About Marknexia",
    ] {
        assert!(
            items.iter().any(|item| item == wanted),
            "{wanted} in {items:?}"
        );
    }
    // The updater waits for owner gate G2.
    assert!(
        !items.iter().any(|item| item.contains("update")),
        "{items:?}"
    );
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn troubleshooting_copies_the_dotnet_diagnostics() {
    let shell = launch(&[], None);
    shell.wait_id("2001", UIA_ButtonControlTypeId, "Toggle sidebar");
    shell.post_command(MENU_TROUBLESHOOTING);
    let dialog = shell.until("the troubleshooting dialog", || {
        shell.find_dialog("Marknexia troubleshooting")
    });
    assert_eq!(
        unsafe { dialog.CurrentControlType() }.unwrap(),
        UIA_WindowControlTypeId
    );
    let minimum = shell.minimum_pixels();
    let mut copy = None;
    for (id, wanted) in [
        ("1", "Copy diagnostics"),
        ("100", "Open data folder"),
        ("2", "Close"),
    ] {
        let button = shell.until(&format!("dialog button {wanted:?}"), || {
            shell
                .find_id_in(&dialog, id)
                .filter(|button| name(button) == wanted)
        });
        assert_target(&button, minimum, false);
        if id == "1" {
            copy = Some(button);
        }
    }
    invoke(&copy.unwrap());
    shell.wait_diagnostic("Informational: Diagnostics copied.");
    let copied = clipboard_text().expect("clipboard text");
    let lines: Vec<&str> = copied.split("\r\n").collect();
    assert_eq!(lines[0], "Marknexia diagnostics");
    for (line, prefix) in lines[1..].iter().zip([
        "Version: ",
        "OS: Microsoft Windows NT ",
        "Architecture: ",
        "App path: ",
        "Local data: ",
        "WebView2 data: ",
        "SBOM present: ",
        "Remote images enabled: False",
    ]) {
        assert!(line.starts_with(prefix), "{line:?} starts with {prefix:?}");
    }
    assert_eq!(lines.len(), 9);
}

/// Removes the test SBOM it wrote beside the shell executable.
struct SbomFile(PathBuf);

impl Drop for SbomFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn the_sbom_item_warns_when_missing_and_copies_the_inventory() {
    let executable = PathBuf::from(std::env::var_os("MARKNEXIA_SHELL_EXE").expect("exe"));
    let path = executable.with_file_name("marknexia-sbom.spdx.json");
    let existing = path.exists();
    if !existing {
        // .NET: "missing" while no inventory ships beside the executable.
        let shell = launch(&[], None);
        shell.wait_id("2001", UIA_ButtonControlTypeId, "Toggle sidebar");
        shell.post_command(MENU_SBOM);
        shell.wait_diagnostic(
            "Warning: The software components inventory is missing. Reinstall Marknexia from its release package.",
        );
    }
    let json = "{\"spdxVersion\":\"SPDX-2.3\",\"packages\":[{\"name\":\"zeta\",\"versionInfo\":\"2\"},{\"name\":\"Alpha\",\"versionInfo\":\"1.0\"}]}";
    let _cleanup = (!existing).then(|| {
        std::fs::write(&path, json).expect("write the test SBOM");
        SbomFile(path.clone())
    });
    let expected = std::fs::read_to_string(&path).expect("read the SBOM");
    let shell = launch(&[], None);
    shell.wait_id("2001", UIA_ButtonControlTypeId, "Toggle sidebar");
    shell.post_command(MENU_SBOM);
    let dialog = shell.until("the software components dialog", || {
        shell.find_dialog("Marknexia software components")
    });
    let list = shell.until("the component list", || {
        shell
            .find_id_in(&dialog, "101")
            .filter(|list| name(list) == "Software components and versions")
    });
    let entries: Vec<String> = shell
        .find_all(&list, UIA_ListItemControlTypeId, true)
        .iter()
        .map(name)
        .collect();
    if !existing {
        assert_eq!(entries, ["Alpha  1.0", "zeta  2"]);
    }
    assert!(!entries.is_empty());
    let minimum = shell.minimum_pixels();
    let copy = shell.until("Copy SBOM", || {
        shell
            .find_id_in(&dialog, "1")
            .filter(|button| name(button) == "Copy SBOM")
    });
    assert_target(&copy, minimum, false);
    assert_target(
        &shell
            .find_id_in(&dialog, "2")
            .filter(|button| name(button) == "Close")
            .expect("Close"),
        minimum,
        false,
    );
    invoke(&copy);
    shell.wait_diagnostic("Informational: Software components inventory copied.");
    assert_eq!(
        clipboard_text().as_deref(),
        Some(expected.trim_start_matches('\u{feff}'))
    );
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn copy_heading_anchor_copies_only_the_anchor() {
    let features = fixture("features.md");
    let shell = launch(&[features.as_os_str()], None);
    let outline = shell.wait_id("2011", UIA_ListControlTypeId, "Document outline");
    let items = shell.until("outline items", || {
        let items = shell.find_all(&outline, UIA_ListItemControlTypeId, true);
        (items.len() > 2).then_some(items)
    });
    let pattern: IUIAutomationSelectionItemPattern =
        unsafe { items[2].GetCurrentPattern(UIA_SelectionItemPatternId) }
            .expect("SelectionItem pattern")
            .cast()
            .expect("SelectionItem interface");
    unsafe { pattern.Select() }.expect("Select");
    let copy = shell.wait_id("2018", UIA_ButtonControlTypeId, "Copy heading anchor");
    shell.until("Copy heading anchor to be enabled", || {
        is_enabled(&copy).then_some(())
    });
    assert!(is_onscreen(&copy));
    assert_target(&copy, shell.minimum_pixels(), true);
    invoke(&copy);
    let notice = shell.wait_diagnostic("Informational: Copied #");
    let anchor = notice
        .strip_prefix("Informational: Copied ")
        .expect("the anchor in the notice");
    assert!(anchor.len() > 1 && !anchor.contains(char::is_whitespace));
    assert_eq!(clipboard_text().as_deref(), Some(anchor));
    // The repository view's button is hidden with its view.
    assert!(
        shell
            .find_id("2019")
            .is_none_or(|reveal| !is_onscreen(&reveal))
    );
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn reveal_in_file_explorer_selects_the_file_in_its_folder() {
    let repository = TempFolder::new("reveal");
    std::fs::write(repository.0.join("README.md"), "# Reveal readme\n").unwrap();
    let settings = format!(
        "{{\"RepositoryRoot\": {}, \"SidebarMode\": 1}}",
        json_string(&repository.0.to_string_lossy())
    );
    let shell = launch(&[], Some(settings.as_bytes()));
    let tree = shell.wait_id("2012", UIA_TreeControlTypeId, "Repository files");
    let root_name = repository
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let root = shell.until("the repository root item", || {
        shell
            .find_all(&tree, UIA_TreeItemControlTypeId, true)
            .into_iter()
            .find(|item| name(item) == root_name)
    });
    let readme = shell.until("README.md in the tree", || {
        shell
            .find_all(&root, UIA_TreeItemControlTypeId, true)
            .into_iter()
            .find(|item| name(item) == "README.md")
    });
    let select: IUIAutomationSelectionItemPattern =
        unsafe { readme.GetCurrentPattern(UIA_SelectionItemPatternId) }
            .expect("SelectionItem pattern")
            .cast()
            .expect("SelectionItem interface");
    unsafe { select.Select() }.expect("Select README.md");
    let reveal = shell.wait_id("2019", UIA_ButtonControlTypeId, "Reveal in File Explorer");
    shell.until("Reveal to be enabled", || is_enabled(&reveal).then_some(()));
    assert_target(&reveal, shell.minimum_pixels(), true);
    let before = top_level_windows("CabinetWClass");
    invoke(&reveal);
    let opened = shell.until("a File Explorer window for the repository", || {
        top_level_windows("CabinetWClass")
            .into_iter()
            .find(|window| !before.contains(window) && window_title(*window) == root_name)
    });
    let _ = unsafe { PostMessageW(Some(opened), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    assert!(
        shell
            .find_id("2020")
            .is_none_or(|text| !name(&text).contains("Could not open File Explorer"))
    );
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn the_dark_menu_bar_is_dark_and_keeps_its_names() {
    use windows::Win32::{
        Graphics::Gdi::{GetDC, GetPixel, ReleaseDC},
        UI::WindowsAndMessaging::{GetMenuBarInfo, MENUBARINFO, OBJID_MENU},
    };
    // Theme 2 is Dark (.NET `AppTheme`).
    let shell = launch(&[], Some(b"{\"Theme\": 2}"));
    shell.wait_id("2001", UIA_ButtonControlTypeId, "Toggle sidebar");
    let menus: Vec<String> = shell.until("the menu bar items", || {
        let names: Vec<String> = shell
            .find_all(&shell.root, UIA_MenuBarControlTypeId, false)
            .iter()
            .flat_map(|bar| {
                shell
                    .find_all(
                        bar,
                        windows::Win32::UI::Accessibility::UIA_MenuItemControlTypeId,
                        true,
                    )
                    .into_iter()
                    .map(|item| name(&item))
                    .collect::<Vec<_>>()
            })
            .collect();
        (names.len() >= 4).then_some(names)
    });
    for menu in ["File", "View", "Settings", "Help"] {
        assert!(menus.iter().any(|item| item == menu), "{menu} in {menus:?}");
    }
    assert!(
        shell.activate(),
        "the shell must be visible to sample the bar"
    );
    let mut info = MENUBARINFO {
        cbSize: std::mem::size_of::<MENUBARINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMenuBarInfo(shell.hwnd, OBJID_MENU, 0, &mut info) }.expect("GetMenuBarInfo");
    let bar: RECT = info.rcBar;
    // The empty bar right of the items shows the bar background.
    let pixel = shell.until("a painted menu bar", || {
        let dc = unsafe { GetDC(None) };
        let color = unsafe { GetPixel(dc, bar.right - 8, (bar.top + bar.bottom) / 2) }.0;
        unsafe { ReleaseDC(None, dc) };
        (color != 0xFFFF_FFFF).then_some(color)
    });
    let channels = [pixel & 0xFF, (pixel >> 8) & 0xFF, (pixel >> 16) & 0xFF];
    eprintln!("menu bar background: #{:06X}", pixel);
    assert!(
        channels.iter().all(|channel| *channel < 0x60),
        "dark menu bar background, got {channels:?}"
    );
}
