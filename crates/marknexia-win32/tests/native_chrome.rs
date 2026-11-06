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
                UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID, UIA_ComboBoxControlTypeId,
                UIA_ControlTypePropertyId, UIA_ExpandCollapsePatternId, UIA_InvokePatternId,
                UIA_ListControlTypeId, UIA_ListItemControlTypeId, UIA_MenuBarControlTypeId,
                UIA_NamePropertyId, UIA_SelectionItemPatternId, UIA_TabControlTypeId,
                UIA_TabItemControlTypeId, UIA_TreeControlTypeId, UIA_TreeItemControlTypeId,
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

    fn wait(&self, kind: UIA_CONTROLTYPE_ID, wanted: &str) -> IUIAutomationElement {
        wait_for(&format!("{wanted:?} ({})", kind.0), || {
            self.find(kind, wanted)
        })
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

    for button in [
        "Toggle sidebar",
        "Back",
        "Forward",
        "Reload document",
        "Open repository folder",
    ] {
        let element = shell.wait(UIA_ButtonControlTypeId, button);
        assert_target(&element, minimum, true);
    }
    let theme = shell.wait(UIA_ComboBoxControlTypeId, "Theme");
    assert_target(&theme, minimum, true);

    let selector = shell.wait(UIA_TabControlTypeId, "Sidebar view");
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
    let outline = shell.wait(UIA_ListControlTypeId, "Document outline");
    let heading = wait_for("the outline's first heading", || {
        shell
            .find_all(&outline, UIA_ListItemControlTypeId, true)
            .into_iter()
            .find(|item| name(item) == "GFM Full Features Test")
    });
    assert_target(&heading, minimum, false);
    // A newly opened document has no history yet; it can be reloaded.
    assert!(!is_enabled(&shell.wait(UIA_ButtonControlTypeId, "Back")));
    assert!(is_enabled(
        &shell.wait(UIA_ButtonControlTypeId, "Reload document")
    ));

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
    let settings = wait_for("the recent file to be saved", || {
        read_settings(&shell.shell).filter(|settings| !settings.recent_files.is_empty())
    });
    assert!(settings.recent_files[0].ends_with("features.md"));
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn toggling_the_sidebar_hides_it_and_persists_the_setting() {
    let features = fixture("features.md");
    let shell = launch(&[features.as_os_str()], None);
    let outline = shell.wait(UIA_ListControlTypeId, "Document outline");
    assert!(is_onscreen(&outline));
    invoke(&shell.wait(UIA_ButtonControlTypeId, "Toggle sidebar"));
    wait_for("IsSidebarOpen=false in the settings file", || {
        read_settings(&shell.shell)
            .filter(|settings| !settings.is_sidebar_open)
            .map(drop)
    });
    wait_for("the outline to leave the screen", || {
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

    let tree = shell.wait(UIA_TreeControlTypeId, "Repository files");
    let root_name = repository
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let root = wait_for("the repository root item", || {
        shell
            .find_all(&tree, UIA_TreeItemControlTypeId, true)
            .into_iter()
            .find(|item| name(item) == root_name)
    });
    let children = wait_for("the root's children", || {
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
    wait_for("guide.md after expansion", || {
        shell
            .find_all(&docs, UIA_TreeItemControlTypeId, true)
            .into_iter()
            .find(|item| name(item) == "guide.md")
    });
    let selector = shell.wait(UIA_TabControlTypeId, "Sidebar view");
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
    let warning = wait_for("the settings warning", || {
        shell
            .find_all(
                &shell.root,
                windows::Win32::UI::Accessibility::UIA_TextControlTypeId,
                false,
            )
            .into_iter()
            .find(|text| name(text).starts_with("Warning: Settings were reset to defaults"))
    });
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
    let close = shell.wait(UIA_ButtonControlTypeId, "Close");
    assert_target(&close, shell.minimum_pixels(), true);
    invoke(&close);
    wait_for("the diagnostics bar to close", || {
        (!is_onscreen(&warning) || name(&warning).is_empty()).then_some(())
    });
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn enter_on_a_heading_jumps_and_alt_left_goes_back() {
    let features = fixture("features.md");
    let shell = launch(&[features.as_os_str()], None);
    let outline = shell.wait(UIA_ListControlTypeId, "Document outline");
    let items = wait_for("outline items", || {
        let items = shell.find_all(&outline, UIA_ListItemControlTypeId, true);
        (items.len() > 2).then_some(items)
    });
    let back = shell.wait(UIA_ButtonControlTypeId, "Back");
    let forward = shell.wait(UIA_ButtonControlTypeId, "Forward");
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
    wait_for("Back to be enabled after the jump", || {
        is_enabled(&back).then_some(())
    });
    send_chord_to(shell.shell.id(), &[VK_MENU, VK_LEFT]);
    wait_for("Forward to be enabled after Alt+Left", || {
        is_enabled(&forward).then_some(())
    });
    assert!(!is_enabled(&back));
}
