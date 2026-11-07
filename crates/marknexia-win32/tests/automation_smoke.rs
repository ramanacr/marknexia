#![cfg(windows)]

//! Controlled native UI Automation gate. The AV-safe source pass does not run
//! this ignored test; `Test-RustNativeShell.ps1` supplies the final executable.

use std::{
    thread,
    time::{Duration, Instant},
};

use windows::{
    Win32::{
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
                IUIAutomationSelectionItemPattern, IUIAutomationTextPattern, TreeScope_Children,
                TreeScope_Descendants, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
                UIA_SelectionItemPatternId, UIA_TabControlTypeId, UIA_TabItemControlTypeId,
                UIA_TextPatternId,
            },
            Input::KeyboardAndMouse::{VK_CONTROL, VK_TAB},
        },
    },
    core::Interface,
};

const TIMEOUT: Duration = Duration::from_secs(15);

/// Launch with a unique WebView2 profile, cleanup, window lookup and guarded
/// key input, shared with `native_accessibility.rs`.
#[path = "support/shell.rs"]
mod shell_support;

use shell_support::{
    ShellProcess, activate_attempt, find_shell_window, foreground_owner, send_chord_to,
};

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

fn visible_document_element(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> Option<IUIAutomationElement> {
    let condition = unsafe {
        automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_DocumentControlTypeId.0),
        )
    }
    .ok()?;
    let found = unsafe { root.FindAll(TreeScope_Descendants, &condition) }.ok()?;
    (0..unsafe { found.Length() }.ok()?)
        .filter_map(|index| unsafe { found.GetElement(index) }.ok())
        .find(|document| {
            unsafe { document.CurrentIsOffscreen() }.is_ok_and(|offscreen| !offscreen.as_bool())
        })
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

/// All text of a document through the UIA Text pattern.
fn document_text(document: &IUIAutomationElement) -> Option<String> {
    let pattern: IUIAutomationTextPattern =
        unsafe { document.GetCurrentPattern(UIA_TextPatternId) }
            .ok()?
            .cast()
            .ok()?;
    let range = unsafe { pattern.DocumentRange() }.ok()?;
    unsafe { range.GetText(-1) }
        .ok()
        .map(|text| text.to_string())
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn command_line_markdown_file_opens_as_a_named_rendered_document() {
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/markdown/gfm/features.md");
    let fixture = std::fs::canonicalize(fixture).expect("features.md fixture");
    let source = std::fs::read_to_string(&fixture).expect("read fixture");
    let heading = source
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .expect("fixture's first heading")
        .trim()
        .to_owned();
    let launched = Instant::now();
    let shell = ShellProcess::launch(&executable, &[fixture.as_os_str()]);
    let hwnd = wait_for("the launched shell HWND", || find_shell_window(shell.id()));

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
    // A document path replaces the start-page tab.
    let tabs = children(&automation, &tab_control);
    let names: Vec<String> = tabs.iter().map(name).collect();
    assert_eq!(names, ["features.md"], "one tab named after the file");

    wait_for("the rendered document named after the file", || {
        visible_document(&automation, &root).filter(|document| document == "features.md")
    });
    let document = wait_for("the visible document element", || {
        visible_document_element(&automation, &root)
    });
    let text = wait_for("the rendered heading text", || {
        document_text(&document).filter(|text| text.contains(&heading))
    });
    // Markdown syntax is rendered, not shown: no raw table or alert markers.
    assert!(!text.contains("[!NOTE]"), "alert syntax must be rendered");
    assert!(!text.contains("| :--- |"), "table syntax must be rendered");
    eprintln!(
        "features.md visible with heading {heading:?} after {} ms",
        launched.elapsed().as_millis()
    );
    let _ = hwnd;
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn two_named_tab_items_expose_selection_and_change_active_tab() {
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    // Two documents give two tabs (the start page is a single tab).
    let documents = shell_support::TwoDocuments::new();
    let shell = ShellProcess::launch(&executable, &documents.args());
    let hwnd = wait_for("the launched shell HWND", || find_shell_window(shell.id()));

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

    // Keyboard handoff: with focus inside the active WebView document, the
    // shell-owned Ctrl+Tab must still switch tabs (AcceleratorKeyPressed).
    let document = wait_for("a focusable visible document", || {
        visible_document_element(&automation, &root)
    });
    // SetForegroundWindow is refused when this test process is not itself in
    // the foreground (e.g. on CI runners). UI Automation SetFocus may activate
    // the target window, so try both until the launched shell owns the
    // foreground.
    // The synthetic Alt unlock inside `activate_attempt` is only used on
    // GitHub Actions runners, never on a developer desktop where a synthetic
    // key could reach another application.
    let shell_is_foreground = || foreground_owner() == shell.id();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut attempt = 0_u32;
    while !shell_is_foreground() && Instant::now() < deadline {
        activate_attempt(hwnd, attempt);
        let _ = unsafe { root.SetFocus() };
        thread::sleep(Duration::from_millis(200));
        attempt += 1;
    }
    unsafe { document.SetFocus() }.expect("focus WebView document");
    thread::sleep(Duration::from_millis(300));
    // Never inject keys unless the launched shell owns the foreground, so a
    // failed activation cannot type into another application; the owner is
    // re-checked immediately before SendInput.
    send_chord_to(shell.id(), &[VK_CONTROL, VK_TAB]);
    let expected = name(&tabs[0]);
    wait_for("Ctrl+Tab from inside the document to switch tabs", || {
        visible_document(&automation, &root).filter(|document| *document == expected)
    });
}
