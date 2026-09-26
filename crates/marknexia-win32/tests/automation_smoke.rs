#![cfg(windows)]

//! Controlled native UI Automation gate. The AV-safe source pass does not run
//! this ignored test; `Test-RustNativeShell.ps1` supplies the final executable.

use std::{
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM},
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
                IUIAutomationSelectionItemPattern, TreeScope_Children, TreeScope_Descendants,
                UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_SelectionItemPatternId,
                UIA_TabControlTypeId, UIA_TabItemControlTypeId,
            },
            WindowsAndMessaging::{
                EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
            },
        },
    },
    core::{BOOL, Interface},
};

const SHELL_CLASS: &str = "MarknexiaRustWindow";
const TIMEOUT: Duration = Duration::from_secs(15);

/// Kills the launched shell even when an assertion fails mid-test.
struct ShellProcess(Child);

impl Drop for ShellProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Single-threaded COM apartment for the UIA client, released on drop.
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
fn find_shell_window(process_id: u32) -> Option<HWND> {
    let mut search = WindowSearch {
        process_id,
        found: None,
    };
    // EnumWindows reports an error when the callback stops early; the result
    // is read from `search` instead.
    let _ = unsafe {
        EnumWindows(
            Some(match_shell_window),
            LPARAM(std::ptr::from_mut(&mut search) as isize),
        )
    };
    search.found
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

fn children(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
) -> Vec<IUIAutomationElement> {
    // Tolerate transient UIA errors (stale elements during startup); callers
    // poll through `wait_for` or assert on the result.
    let Ok(condition) = (unsafe { automation.CreateTrueCondition() }) else {
        return Vec::new();
    };
    let Ok(found) = (unsafe { element.FindAll(TreeScope_Children, &condition) }) else {
        return Vec::new();
    };
    let length = unsafe { found.Length() }.unwrap_or(0);
    (0..length)
        .filter_map(|index| unsafe { found.GetElement(index) }.ok())
        .collect()
}

fn name(element: &IUIAutomationElement) -> String {
    unsafe { element.CurrentName() }
        .map(|name| name.to_string())
        .unwrap_or_default()
}

/// Name of the on-screen WebView document. Hidden controllers belong to
/// non-selected tabs and must not count as the active view.
fn visible_document(automation: &IUIAutomation, root: &IUIAutomationElement) -> Option<String> {
    let condition = unsafe {
        automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_DocumentControlTypeId.0),
        )
    }
    .ok()?;
    let found = unsafe { root.FindAll(TreeScope_Descendants, &condition) }.ok()?;
    let mut visible = (0..unsafe { found.Length() }.ok()?)
        .filter_map(|index| unsafe { found.GetElement(index) }.ok())
        .filter(|document| {
            unsafe { document.CurrentIsOffscreen() }.is_ok_and(|offscreen| !offscreen.as_bool())
        })
        .map(|document| name(&document))
        .filter(|name| !name.is_empty());
    let first = visible.next()?;
    // Exactly one controller may be visible at a time.
    visible.next().is_none().then_some(first)
}

fn select_and_observe(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    tab: &IUIAutomationElement,
) {
    let pattern: IUIAutomationSelectionItemPattern =
        unsafe { tab.GetCurrentPattern(UIA_SelectionItemPatternId) }
            .expect("SelectionItem pattern")
            .cast()
            .expect("SelectionItem interface");
    unsafe { pattern.Select() }.expect("Select");
    // Select is synchronous: selection state must already agree on return.
    assert!(unsafe { pattern.CurrentIsSelected() }.unwrap().as_bool());
    let expected = name(tab);
    wait_for("the selected tab's WebView document", || {
        visible_document(automation, root).filter(|document| *document == expected)
    });
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn two_named_tab_items_expose_selection_and_change_active_tab() {
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let shell = ShellProcess(Command::new(executable).spawn().expect("launch shell"));
    let hwnd = wait_for("the launched shell HWND", || {
        find_shell_window(shell.0.id())
    });

    let _apartment = Apartment::enter();
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .expect("CUIAutomation");
    let root = unsafe { automation.ElementFromHandle(hwnd) }.expect("ElementFromHandle");
    let tab_control = wait_for("the custom TabControl", || {
        children(&automation, &root).into_iter().find(|element| {
            unsafe { element.CurrentControlType() }.is_ok_and(|kind| kind == UIA_TabControlTypeId)
        })
    });
    let tabs = children(&automation, &tab_control);
    assert_eq!(
        tabs.len(),
        2,
        "custom TabItem children; default HWND proxy is insufficient"
    );
    for tab in &tabs {
        assert_eq!(
            unsafe { tab.CurrentControlType() }.unwrap(),
            UIA_TabItemControlTypeId
        );
        assert!(!name(tab).is_empty());
    }
    assert_ne!(name(&tabs[0]), name(&tabs[1]));

    // The shell opens with the last tab active, so selecting the first tab
    // and then the second proves two real active-view changes.
    select_and_observe(&automation, &root, &tabs[0]);
    select_and_observe(&automation, &root, &tabs[1]);
}
