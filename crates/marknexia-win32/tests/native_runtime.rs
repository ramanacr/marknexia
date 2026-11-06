#![cfg(windows)]

//! Native gate for the WebView2 missing-runtime state (Phase 5).
//!
//! The shell is launched with `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` pointing
//! at a folder that does not exist, so the loader reports no runtime while
//! the installed runtime stays untouched. The test only reads UIA: it never
//! presses Install, so nothing is downloaded or run, and it sends no keys.
//!
//! Ignored by default: it needs `MARKNEXIA_SHELL_EXE` and an interactive
//! desktop (not the lock screen).

use std::{
    thread,
    time::{Duration, Instant},
};

use marknexia_webview::bootstrap::{MISSING_DIAGNOSTIC, MISSING_MESSAGE};
use windows::{
    Win32::{
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Variant::VARIANT,
        },
        UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Descendants,
            UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId, UIA_ClassNamePropertyId,
        },
    },
    core::BSTR,
};

#[path = "support/shell.rs"]
mod shell_support;

use shell_support::{ShellProcess, find_shell_window};

const TIMEOUT: Duration = Duration::from_secs(20);

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

fn name(element: &IUIAutomationElement) -> String {
    unsafe { element.CurrentName() }
        .map(|name| name.to_string())
        .unwrap_or_default()
}

fn on_screen(element: &IUIAutomationElement) -> bool {
    unsafe { element.CurrentIsOffscreen() }.is_ok_and(|offscreen| !offscreen.as_bool())
}

fn find_by(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    property: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
    value: &str,
) -> Option<IUIAutomationElement> {
    let condition =
        unsafe { automation.CreatePropertyCondition(property, &VARIANT::from(BSTR::from(value))) }
            .ok()?;
    unsafe { root.FindFirst(TreeScope_Descendants, &condition) }.ok()
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and an interactive desktop"]
fn a_missing_runtime_shows_the_install_state_and_keeps_the_shell_usable() {
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let shell = ShellProcess::launch_with_browser_folder(&executable, &[], "no-webview2-runtime");
    let hwnd = wait_for("the launched shell HWND", || find_shell_window(shell.id()));
    let _apartment = Apartment::enter();
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .expect("CUIAutomation");
    let root = unsafe { automation.ElementFromHandle(hwnd) }.expect("ElementFromHandle");
    let by_id = |id: &str| find_by(&automation, &root, UIA_AutomationIdPropertyId, id);

    // The document area explains the state.
    let text = wait_for("the missing-runtime text", || {
        by_id("2030").filter(|element| on_screen(element) && name(element) == MISSING_MESSAGE)
    });
    assert_eq!(name(&text), MISSING_MESSAGE);

    // The explicit install action, visible and enabled; never pressed here.
    let install = wait_for("the Install WebView2 Runtime button", || {
        by_id("2031").filter(on_screen)
    });
    assert_eq!(name(&install), "Install WebView2 Runtime");
    assert_eq!(
        unsafe { install.CurrentControlType() }.expect("control type"),
        UIA_ButtonControlTypeId
    );
    assert!(
        unsafe { install.CurrentIsEnabled() }
            .expect("enabled")
            .as_bool()
    );
    // No download is running, so there is nothing to cancel.
    assert!(by_id("2032").is_none_or(|cancel| !on_screen(&cancel)));

    // The offline installer link.
    let link = wait_for("the offline installer link", || {
        by_id("2033").filter(on_screen)
    });
    assert!(
        name(&link).contains("offline installer"),
        "link name {:?}",
        name(&link)
    );

    // The diagnostics bar says the same.
    let diagnostic = wait_for("the missing-runtime diagnostic", || {
        by_id("2020").filter(|element| name(element).contains(MISSING_DIAGNOSTIC))
    });
    assert!(on_screen(&diagnostic));

    // The shell stays usable: the Open command is there and enabled, and no
    // WebView2 window was created.
    let open = wait_for("the Open Markdown button", || {
        by_id("1001").filter(on_screen)
    });
    assert!(
        unsafe { open.CurrentIsEnabled() }
            .expect("enabled")
            .as_bool()
    );
    assert!(
        find_by(
            &automation,
            &root,
            UIA_ClassNamePropertyId,
            "Chrome_WidgetWin_0"
        )
        .is_none(),
        "no WebView2 host window without a runtime"
    );
    assert!(
        find_shell_window(shell.id()).is_some(),
        "the shell is still running"
    );
}
