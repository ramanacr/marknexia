#![cfg(windows)]

//! Controlled native accessibility gates for Task 6: per-monitor-v2 DPI
//! awareness and 44-DIP tab targets, UIA structure events on tab close, and
//! the high-contrast palette selection. The AV-safe source pass does not run
//! these ignored tests; `Test-RustNativeGates.ps1` supplies the executable
//! through `MARKNEXIA_SHELL_EXE`.
//!
//! No test here changes a system setting. High contrast is only read
//! (`SPI_GETHIGHCONTRAST`), never set.

use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use marknexia_win32::{
    accessibility::tab_slot,
    theme::{ColorSpec, EffectiveTheme, Palette, palette},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            COLOR_BTNFACE, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
            GetDC, GetPixel, GetSysColor, HGDIOBJ, MapWindowPoints, ReleaseDC, SelectObject,
        },
        Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, SAFEARRAY,
            },
            Variant::VARIANT,
        },
        UI::{
            Accessibility::{
                CUIAutomation, HCF_HIGHCONTRASTON, HIGHCONTRASTW, IUIAutomation,
                IUIAutomationElement, IUIAutomationInvokePattern,
                IUIAutomationSelectionItemPattern, IUIAutomationStructureChangedEventHandler,
                IUIAutomationStructureChangedEventHandler_Impl, StructureChangeType,
                StructureChangeType_ChildRemoved, StructureChangeType_ChildrenInvalidated,
                TreeScope_Children, TreeScope_Descendants, TreeScope_Subtree,
                UIA_ButtonControlTypeId, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
                UIA_InvokePatternId, UIA_SelectionItemPatternId, UIA_TabControlTypeId,
                UIA_TabItemControlTypeId,
            },
            HiDpi::{
                AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                DPI_AWARENESS_PER_MONITOR_AWARE, GetAwarenessFromDpiAwarenessContext,
                GetDpiForWindow, GetWindowDpiAwarenessContext, SetThreadDpiAwarenessContext,
            },
            WindowsAndMessaging::{
                FindWindowExW, GetClientRect, SMTO_ABORTIFHUNG, SPI_GETHIGHCONTRAST,
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageTimeoutW, SystemParametersInfoW,
                WM_NULL,
            },
        },
    },
    core::{Interface, w},
};
use windows_core::{Ref, implement};

const TIMEOUT: Duration = Duration::from_secs(15);
/// The design target from `layout::Dip::INTERACTION_TARGET`, in DIPs.
const TARGET_DIP: f64 = 44.0;
/// `PW_CLIENTONLY | PW_RENDERFULLCONTENT`: the DWM-composed client area,
/// including the owner-drawn tab strip child.
const PRINT_CLIENT_FULL_CONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(1 | 2);

// ---------------------------------------------------------------------------
// Helpers. Launch, cleanup, window lookup and guarded key input are shared
// with `automation_smoke.rs` through `support/shell.rs`.
// ---------------------------------------------------------------------------

#[path = "support/shell.rs"]
mod shell_support;

use shell_support::{ShellProcess, find_shell_window};

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

/// Name of the single on-screen WebView document.
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
    visible.next().is_none().then_some(first)
}

fn select(tab: &IUIAutomationElement) {
    let pattern: IUIAutomationSelectionItemPattern =
        unsafe { tab.GetCurrentPattern(UIA_SelectionItemPatternId) }
            .expect("SelectionItem pattern")
            .cast()
            .expect("SelectionItem interface");
    unsafe { pattern.Select() }.expect("Select");
    assert!(unsafe { pattern.CurrentIsSelected() }.unwrap().as_bool());
}

// ---------------------------------------------------------------------------
// Shared launch for the tests below.
// ---------------------------------------------------------------------------

struct Launched {
    // Field order is drop order: UIA objects go before the apartment, and the
    // shell process is killed last.
    tab_control: IUIAutomationElement,
    root: IUIAutomationElement,
    automation: IUIAutomation,
    _apartment: Apartment,
    hwnd: HWND,
    _shell: ShellProcess,
}

/// Launches the shell with no document (two placeholder tabs) and binds the
/// UIA client to its custom tab control. The test thread is made
/// per-monitor-v2 aware first, so UIA rectangles and Win32 metrics read here
/// are physical pixels, the same unit the shell uses.
fn launch_shell() -> Launched {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let shell = ShellProcess::launch(&executable, &[]);
    let hwnd = wait_for("the launched shell HWND", || find_shell_window(shell.id()));
    let apartment = Apartment::enter();
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .expect("CUIAutomation");
    let root = unsafe { automation.ElementFromHandle(hwnd) }.expect("ElementFromHandle");
    let tab_control = wait_for("the custom TabControl", || {
        children(&automation, &root).into_iter().find(|element| {
            unsafe { element.CurrentControlType() }.is_ok_and(|kind| kind == UIA_TabControlTypeId)
        })
    });
    wait_for("two TabItem children", || {
        (children(&automation, &tab_control).len() == 2).then_some(())
    });
    Launched {
        tab_control,
        root,
        automation,
        _apartment: apartment,
        hwnd,
        _shell: shell,
    }
}

impl Launched {
    fn tabs(&self) -> Vec<IUIAutomationElement> {
        children(&self.automation, &self.tab_control)
    }
}

// ---------------------------------------------------------------------------
// 1. DPI awareness and target size
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn shell_is_per_monitor_v2_and_tab_items_meet_the_44_dip_target() {
    let shell = launch_shell();

    let context = unsafe { GetWindowDpiAwarenessContext(shell.hwnd) };
    assert!(
        unsafe {
            AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        }
        .as_bool(),
        "shell window must be DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2"
    );
    assert_eq!(
        unsafe { GetAwarenessFromDpiAwarenessContext(context) },
        DPI_AWARENESS_PER_MONITOR_AWARE
    );

    let dpi = unsafe { GetDpiForWindow(shell.hwnd) };
    assert!(dpi >= 96, "GetDpiForWindow returned {dpi}");
    let minimum = TARGET_DIP * f64::from(dpi) / 96.0;
    eprintln!(
        "shell DPI {dpi} (scale {}%), minimum target {minimum} px",
        dpi * 100 / 96
    );

    let tabs = shell.tabs();
    assert_eq!(tabs.len(), 2);
    for tab in &tabs {
        assert_eq!(
            unsafe { tab.CurrentControlType() }.unwrap(),
            UIA_TabItemControlTypeId
        );
        let rect = unsafe { tab.CurrentBoundingRectangle() }.expect("BoundingRectangle");
        let width = f64::from(rect.right - rect.left);
        let height = f64::from(rect.bottom - rect.top);
        eprintln!(
            "TabItem {:?}: {width} x {height} px at ({}, {})",
            name(tab),
            rect.left,
            rect.top
        );
        assert!(
            width >= minimum && height >= minimum,
            "TabItem {:?} is {width} x {height} px; 44 DIP at {dpi} DPI is {minimum} px",
            name(tab)
        );
    }
}

// ---------------------------------------------------------------------------
// 2. UIA structure events on tab close
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct StructureEvent {
    change: StructureChangeType,
    sender_is_tab_control: bool,
}

/// Records structure-change events. UIA calls it on its own thread.
#[implement(IUIAutomationStructureChangedEventHandler)]
struct StructureRecorder {
    events: Arc<Mutex<Vec<StructureEvent>>>,
}

impl IUIAutomationStructureChangedEventHandler_Impl for StructureRecorder_Impl {
    fn HandleStructureChangedEvent(
        &self,
        sender: Ref<IUIAutomationElement>,
        changetype: StructureChangeType,
        _runtimeid: *const SAFEARRAY,
    ) -> windows_core::Result<()> {
        let sender_is_tab_control = sender.ok().is_ok_and(|element| {
            unsafe { element.CurrentControlType() }.is_ok_and(|kind| kind == UIA_TabControlTypeId)
        });
        if let Ok(mut events) = self.events.lock() {
            events.push(StructureEvent {
                change: changetype,
                sender_is_tab_control,
            });
        }
        Ok(())
    }
}

/// Removes every UIA handler this client registered before COM shuts down.
struct HandlerRegistration<'a>(&'a IUIAutomation);

impl Drop for HandlerRegistration<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.0.RemoveAllEventHandlers() };
    }
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn closing_a_tab_raises_a_structure_event_and_leaves_no_stale_child() {
    let shell = launch_shell();
    let tabs = shell.tabs();
    assert_eq!(tabs.len(), 2);
    let names: Vec<String> = tabs.iter().map(name).collect();
    assert_ne!(names[0], names[1]);

    // Make the first tab active and wait until its WebView document shows,
    // so the close path below switches between two live controllers.
    select(&tabs[0]);
    wait_for("the first tab's WebView document", || {
        visible_document(&shell.automation, &shell.root).filter(|document| *document == names[0])
    });

    let events = Arc::new(Mutex::new(Vec::new()));
    let handler: IUIAutomationStructureChangedEventHandler = StructureRecorder {
        events: Arc::clone(&events),
    }
    .into();
    unsafe {
        shell.automation.AddStructureChangedEventHandler(
            &shell.tab_control,
            TreeScope_Subtree,
            None,
            &handler,
        )
    }
    .expect("AddStructureChangedEventHandler");
    let _registration = HandlerRegistration(&shell.automation);

    // Close through UIA: every TabItem has exactly one child, its close
    // button, with the Invoke pattern. No synthetic input is needed.
    let close_buttons: Vec<IUIAutomationElement> = tabs
        .iter()
        .map(|tab| {
            let buttons = children(&shell.automation, tab);
            assert_eq!(buttons.len(), 1, "one close button per TabItem");
            let button = buttons.into_iter().next().expect("close button");
            assert_eq!(
                unsafe { button.CurrentControlType() }.unwrap(),
                UIA_ButtonControlTypeId
            );
            assert_eq!(name(&button), "Close Tab");
            button
        })
        .collect();
    let invoke: IUIAutomationInvokePattern =
        unsafe { close_buttons[0].GetCurrentPattern(UIA_InvokePatternId) }
            .expect("Invoke pattern")
            .cast()
            .expect("Invoke interface");
    // Only events raised by the close below count. Wait until earlier
    // events (selection, focus) have stopped arriving for 500 ms, then drop
    // them.
    let quiet = Duration::from_millis(500);
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = events.lock().expect("event list").len();
    let mut stable_since = Instant::now();
    while stable_since.elapsed() < quiet && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
        let now = events.lock().expect("event list").len();
        if now != seen {
            seen = now;
            stable_since = Instant::now();
        }
    }
    events.lock().expect("event list").clear();
    // The shell queues the close and runs it on its next message-loop turn.
    unsafe { invoke.Invoke() }.expect("Invoke on the close button");

    let observed = wait_for("a ChildRemoved or ChildrenInvalidated event", || {
        let events = events.lock().ok()?;
        events
            .iter()
            .find(|event| {
                event.sender_is_tab_control
                    && (event.change == StructureChangeType_ChildRemoved
                        || event.change == StructureChangeType_ChildrenInvalidated)
            })
            .copied()
    });
    eprintln!("structure event: {observed:?}");

    // One fewer TabItem, and the survivor is the other tab.
    let remaining = wait_for("one TabItem after the close", || {
        let remaining = shell.tabs();
        (remaining.len() == 1).then_some(remaining)
    });
    assert_eq!(name(&remaining[0]), names[1]);
    assert!(
        remaining.iter().all(|tab| name(tab) != names[0]),
        "the closed tab's name must be gone from the tab items"
    );
    // The raw view agrees with FindAll: no stale sibling through navigation.
    let walker = unsafe { shell.automation.RawViewWalker() }.expect("RawViewWalker");
    let first = unsafe { walker.GetFirstChildElement(&shell.tab_control) }.expect("first child");
    assert_eq!(name(&first), names[1]);
    assert!(
        unsafe { walker.GetNextSiblingElement(&first) }.is_err(),
        "no sibling after the only remaining tab"
    );
    // The closed tab's element is dead, not a stale live child.
    assert!(
        unsafe { tabs[0].CurrentName() }.is_err(),
        "the closed TabItem must report UIA_E_ELEMENTNOTAVAILABLE"
    );
    assert!(
        unsafe { close_buttons[0].CurrentName() }.is_err(),
        "the closed tab's close button must report UIA_E_ELEMENTNOTAVAILABLE"
    );
}

// ---------------------------------------------------------------------------
// 3. High contrast follows SPI_GETHIGHCONTRAST (read only)
// ---------------------------------------------------------------------------

fn high_contrast_is_on() -> bool {
    let mut value = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            value.cbSize,
            Some(std::ptr::from_mut(&mut value).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .expect("SPI_GETHIGHCONTRAST");
    value.dwFlags.0 & HCF_HIGHCONTRASTON.0 != 0
}

/// Resolves a palette color the way the shell paints it (`COLORREF` order).
fn colorref(spec: ColorSpec) -> u32 {
    match spec {
        ColorSpec::Rgb(rgb) => ((rgb & 0xff) << 16) | (rgb & 0x00ff00) | ((rgb >> 16) & 0xff),
        ColorSpec::System(_) => unreachable!("only brand palettes are resolved here"),
    }
}

/// (inactive tab fill, active tab fill) as the shell paints them.
fn tab_fills(colors: Palette) -> (u32, u32) {
    (colorref(colors.surface), colorref(colors.elevated))
}

/// Samples the inactive and active tab fills from a DWM capture of the
/// shell's client area. Tab 0 must be the active tab.
fn sample_tab_fills(shell_hwnd: HWND, strip: HWND, dpi: u32) -> Option<(u32, u32)> {
    // PrintWindow sends WM_PRINT synchronously and would block on a hung
    // shell: first prove its UI thread answers within five seconds.
    let mut reply = 0_usize;
    let responsive = unsafe {
        SendMessageTimeoutW(
            shell_hwnd,
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            // Generous for slower ARM64 runners.
            5000,
            Some(&mut reply),
        )
    };
    if responsive.0 == 0 {
        return None;
    }
    let mut client = RECT::default();
    unsafe { GetClientRect(shell_hwnd, &mut client) }.ok()?;
    let mut strip_rect = RECT::default();
    unsafe { GetClientRect(strip, &mut strip_rect) }.ok()?;
    let mut origin = [POINT::default()];
    unsafe { MapWindowPoints(Some(strip), Some(shell_hwnd), &mut origin) };

    let screen = unsafe { GetDC(None) };
    let memory = unsafe { CreateCompatibleDC(Some(screen)) };
    let bitmap = unsafe { CreateCompatibleBitmap(screen, client.right, client.bottom) };
    let previous = unsafe { SelectObject(memory, HGDIOBJ(bitmap.0)) };
    let printed = unsafe { PrintWindow(shell_hwnd, memory, PRINT_CLIENT_FULL_CONTENT) }.as_bool();

    // Away from the slot edges (focus rectangle) and the title text.
    let inset = (6 * dpi / 96).max(4) as i32;
    let y = origin[0].y + strip_rect.bottom - inset;
    let pixel_at = |index: usize| {
        let (_, right) = tab_slot(strip_rect.right, 2, index)?;
        Some(unsafe { GetPixel(memory, origin[0].x + right - inset, y) }.0)
    };
    let fills = printed
        .then(|| Some((pixel_at(1)?, pixel_at(0)?)))
        .flatten();

    unsafe {
        SelectObject(memory, previous);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
    }
    fills
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn palette_selection_follows_the_current_high_contrast_state() {
    let high_contrast = high_contrast_is_on();
    let shell = launch_shell();
    let tabs = shell.tabs();
    select(&tabs[0]);
    let dpi = unsafe { GetDpiForWindow(shell.hwnd) };
    let strip = unsafe { FindWindowExW(Some(shell.hwnd), None, w!("MarknexiaRustTabStrip"), None) }
        .expect("MarknexiaRustTabStrip child");

    let brand =
        [EffectiveTheme::Light, EffectiveTheme::Dark].map(|theme| tab_fills(palette(theme)));
    let button_face = unsafe { GetSysColor(COLOR_BTNFACE) };
    // The high-contrast palette fills both inactive and active tabs with
    // COLOR_BTNFACE; each brand palette uses two different colors.
    let contrast = (button_face, button_face);
    let expected: Vec<(u32, u32)> = if high_contrast {
        vec![contrast]
    } else {
        brand.to_vec()
    };

    let deadline = Instant::now() + TIMEOUT;
    let mut last = None;
    while Instant::now() < deadline {
        last = sample_tab_fills(shell.hwnd, strip, dpi);
        if last.is_some_and(|fills| expected.contains(&fills)) {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    eprintln!(
        "SPI_GETHIGHCONTRAST on: {high_contrast}; COLOR_BTNFACE {button_face:#08x}; \
         sampled (inactive, active) tab fills {last:x?}"
    );
    let fills = last.expect("PrintWindow capture of the tab strip");
    assert!(
        expected.contains(&fills),
        "tab fills {fills:x?} do not match the expected palette(s) {expected:x?}"
    );
    if !high_contrast {
        assert_ne!(
            fills, contrast,
            "high-contrast palette used while high contrast is off"
        );
    }
}
