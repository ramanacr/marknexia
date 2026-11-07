#![cfg(windows)]

//! Controlled native gates for Phase 2 tasks 3 to 5: tab close through UIA
//! `Invoke`, single-instance hand-off, find in page through the bridge, and
//! drag reorder. All tests are ignored; they need `MARKNEXIA_SHELL_EXE` and
//! an unlocked interactive desktop (or CI). Synthetic keys and mouse input
//! are sent only after asserting that the launched shell owns the
//! foreground, immediately before each `SendInput`.
//!
//! Drag and drop of files is not covered natively: a real OLE drag needs a
//! drag source driven by the mouse across process boundaries. Its rules are
//! covered by the headless `dragdrop` unit tests.

use std::{
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::HWND,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, SAFEARRAY,
            },
            Variant::VARIANT,
        },
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
                IUIAutomationSelectionItemPattern, IUIAutomationStructureChangedEventHandler,
                IUIAutomationStructureChangedEventHandler_Impl, IUIAutomationValuePattern,
                StructureChangeType, StructureChangeType_ChildRemoved,
                StructureChangeType_ChildrenInvalidated, StructureChangeType_ChildrenReordered,
                TreeScope_Children, TreeScope_Descendants, TreeScope_Subtree,
                UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId, UIA_InvokePatternId,
                UIA_SelectionItemPatternId, UIA_TabControlTypeId, UIA_ValuePatternId,
            },
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN,
                MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput, VK_CONTROL, VK_ESCAPE,
                VK_F,
            },
            WindowsAndMessaging::SetCursorPos,
        },
    },
    core::{BSTR, Interface},
};
use windows_core::{Ref, implement};

#[path = "support/shell.rs"]
mod shell_support;

use shell_support::{
    ShellProcess, activate_attempt, find_shell_window, foreground_owner, send_chord_to,
    unique_instance_key,
};

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

/// A Win32 child control by its control ID (the MSAA proxy's AutomationId).
fn by_control_id(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    id: u32,
) -> Option<IUIAutomationElement> {
    let condition = unsafe {
        automation.CreatePropertyCondition(
            UIA_AutomationIdPropertyId,
            &VARIANT::from(BSTR::from(id.to_string())),
        )
    }
    .ok()?;
    unsafe { root.FindFirst(TreeScope_Descendants, &condition) }.ok()
}

fn invoke(element: &IUIAutomationElement) {
    let pattern: IUIAutomationInvokePattern =
        unsafe { element.GetCurrentPattern(UIA_InvokePatternId) }
            .expect("Invoke pattern")
            .cast()
            .expect("Invoke interface");
    unsafe { pattern.Invoke() }.expect("Invoke");
}

struct Launched {
    tab_control: IUIAutomationElement,
    root: IUIAutomationElement,
    automation: IUIAutomation,
    _apartment: Apartment,
    hwnd: HWND,
    shell: ShellProcess,
    /// The opened documents, removed after the shell exits.
    _documents: Option<shell_support::TwoDocuments>,
}

/// Launches the shell with two documents: two tabs (without documents the
/// shell starts with the single start-page tab).
fn launch_two_documents(key: &str) -> Launched {
    let documents = shell_support::TwoDocuments::new();
    let mut launched = launch(&documents.args(), key, 2);
    launched._documents = Some(documents);
    launched
}

fn launch(args: &[&std::ffi::OsStr], key: &str, tabs: usize) -> Launched {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let shell = ShellProcess::launch_with_key(&executable, args, key);
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
    wait_for("the startup TabItems", || {
        (children(&automation, &tab_control).len() == tabs).then_some(())
    });
    Launched {
        tab_control,
        root,
        automation,
        _apartment: apartment,
        hwnd,
        shell,
        _documents: None,
    }
}

impl Launched {
    fn tabs(&self) -> Vec<IUIAutomationElement> {
        children(&self.automation, &self.tab_control)
    }

    fn tab_names(&self) -> Vec<String> {
        self.tabs().iter().map(name).collect()
    }

    fn selected_name(&self) -> Option<String> {
        self.tabs().into_iter().find_map(|tab| {
            let pattern: IUIAutomationSelectionItemPattern =
                unsafe { tab.GetCurrentPattern(UIA_SelectionItemPatternId) }
                    .ok()?
                    .cast()
                    .ok()?;
            unsafe { pattern.CurrentIsSelected() }
                .ok()?
                .as_bool()
                .then(|| name(&tab))
        })
    }

    /// Activates the shell; the caller still re-checks before each input.
    fn activate(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut attempt = 0;
        while foreground_owner() != self.shell.id() && Instant::now() < deadline {
            activate_attempt(self.hwnd, attempt);
            attempt += 1;
            let _ = unsafe { self.root.SetFocus() };
            thread::sleep(Duration::from_millis(200));
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct StructureEvent {
    change: StructureChangeType,
}

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
        let from_tab_control = sender.ok().is_ok_and(|element| {
            unsafe { element.CurrentControlType() }.is_ok_and(|kind| kind == UIA_TabControlTypeId)
        });
        if from_tab_control && let Ok(mut events) = self.events.lock() {
            events.push(StructureEvent { change: changetype });
        }
        Ok(())
    }
}

struct HandlerRegistration<'a>(&'a IUIAutomation);

impl Drop for HandlerRegistration<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.0.RemoveAllEventHandlers() };
    }
}

fn record_structure(
    shell: &Launched,
) -> (Arc<Mutex<Vec<StructureEvent>>>, HandlerRegistration<'_>) {
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
    (events, HandlerRegistration(&shell.automation))
}

/// A fresh temporary folder with Markdown fixtures, removed on drop.
struct Fixtures(PathBuf);

impl Fixtures {
    fn new(files: &[(&str, &str)]) -> Self {
        let folder = std::env::temp_dir().join(format!("mnx-input-{}", unique_instance_key()));
        std::fs::create_dir(&folder).expect("fixture folder");
        let folder = std::path::absolute(&folder).expect("absolute fixture folder");
        for (file, text) in files {
            std::fs::write(folder.join(file), text).expect("fixture file");
        }
        Self(folder)
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// 1. Tab close through UIA Invoke (no synthetic input)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and an interactive desktop"]
fn invoking_a_close_button_closes_that_inactive_tab() {
    let shell = launch_two_documents(&unique_instance_key());
    let names = shell.tab_names();
    // The second document is active; close the first, inactive one.
    assert_eq!(shell.selected_name().as_deref(), Some(names[1].as_str()));
    let (events, _registration) = record_structure(&shell);
    let tabs = shell.tabs();
    let buttons = children(&shell.automation, &tabs[0]);
    assert_eq!(buttons.len(), 1);
    assert_eq!(
        unsafe { buttons[0].CurrentControlType() }.unwrap(),
        UIA_ButtonControlTypeId
    );
    assert!(
        !unsafe { buttons[0].CurrentIsKeyboardFocusable() }
            .unwrap()
            .as_bool()
    );
    invoke(&buttons[0]);
    wait_for("one TabItem", || (shell.tabs().len() == 1).then_some(()));
    assert_eq!(shell.tab_names(), [names[1].clone()]);
    assert_eq!(shell.selected_name().as_deref(), Some(names[1].as_str()));
    wait_for("ChildRemoved or ChildrenInvalidated", || {
        events
            .lock()
            .ok()?
            .iter()
            .any(|event| {
                event.change == StructureChangeType_ChildRemoved
                    || event.change == StructureChangeType_ChildrenInvalidated
            })
            .then_some(())
    });
    assert!(
        unsafe { buttons[0].CurrentName() }.is_err(),
        "stale close button"
    );
}

// ---------------------------------------------------------------------------
// 2. Single instance (no synthetic input)
// ---------------------------------------------------------------------------

fn second_launch(key: &str, args: &[&std::ffi::OsStr], directory: &std::path::Path) -> bool {
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let mut child = Command::new(executable)
        .args(args)
        .current_dir(directory)
        .env("MARKNEXIA_INSTANCE_KEY", key)
        .spawn()
        .expect("second launch");
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("second launch status") {
            return status.success();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the second launch did not hand off and exit");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and an interactive desktop"]
fn a_second_launch_hands_its_file_to_the_running_instance() {
    let fixtures = Fixtures::new(&[("first.md", "# First\n"), ("second.md", "# Second\n")]);
    let key = unique_instance_key();
    // The start page is the only tab without documents.
    let shell = launch(&[], &key, 1);
    let first = fixtures.0.join("first.md");

    assert!(second_launch(
        &key,
        &[first.as_os_str()],
        &std::env::temp_dir()
    ));
    wait_for("first.md in the running instance", || {
        (shell.tab_names().last().map(String::as_str) == Some("first.md")).then_some(())
    });
    assert_eq!(shell.selected_name().as_deref(), Some("first.md"));
    // The first document takes over the start-page tab (.NET hides its
    // welcome panel; CHROME-27), so "Welcome" does not stay beside it.
    assert_eq!(shell.tab_names(), ["first.md"]);

    // A relative argument resolves against the second launch's directory.
    assert!(second_launch(&key, &["second.md".as_ref()], &fixtures.0));
    wait_for("second.md in the running instance", || {
        (shell.tab_names().last().map(String::as_str) == Some("second.md")).then_some(())
    });

    // An already open file selects its tab instead of opening another.
    assert!(second_launch(
        &key,
        &[first.as_os_str()],
        &std::env::temp_dir()
    ));
    wait_for("first.md selected again", || {
        (shell.selected_name().as_deref() == Some("first.md")).then_some(())
    });
    assert_eq!(shell.tabs().len(), 2);
}

// ---------------------------------------------------------------------------
// 3. Find in page through the bridge (Ctrl+F and Escape are synthetic keys)
// ---------------------------------------------------------------------------

const FIND_EDIT_ID: u32 = 1004;
const FIND_PREVIOUS_ID: u32 = 1006;
const FIND_NEXT_ID: u32 = 1007;
const FIND_COUNT_ID: u32 = 1010;

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn find_in_page_counts_and_steps_through_rendered_matches() {
    let fixtures = Fixtures::new(&[(
        "find.md",
        "# Find\n\nalpha beta **alpha** gamma\n\n- alpha\n- delta\n",
    )]);
    let document = fixtures.0.join("find.md");
    let shell = launch(&[document.as_os_str()], &unique_instance_key(), 1);
    let count = || {
        by_control_id(&shell.automation, &shell.root, FIND_COUNT_ID)
            .map(|label| name(&label))
            .unwrap_or_default()
    };

    shell.activate();
    send_chord_to(shell.shell.id(), &[VK_CONTROL, VK_F]);
    let edit = wait_for("the visible find box", || {
        by_control_id(&shell.automation, &shell.root, FIND_EDIT_ID).filter(|edit| {
            unsafe { edit.CurrentIsOffscreen() }.is_ok_and(|offscreen| !offscreen.as_bool())
        })
    });
    assert_eq!(name(&edit), "Find in document");
    let value: IUIAutomationValuePattern = unsafe { edit.GetCurrentPattern(UIA_ValuePatternId) }
        .expect("Value pattern")
        .cast()
        .expect("Value interface");
    unsafe { value.SetValue(&BSTR::from("Alpha")) }.expect("SetValue");

    let next = by_control_id(&shell.automation, &shell.root, FIND_NEXT_ID).expect("next button");
    let previous =
        by_control_id(&shell.automation, &shell.root, FIND_PREVIOUS_ID).expect("previous button");
    // The first request may arrive before the document finished loading.
    wait_for("the first match", || {
        invoke(&next);
        thread::sleep(Duration::from_millis(300));
        (count() == "1 of 3").then_some(())
    });
    invoke(&next);
    wait_for("the second match", || (count() == "2 of 3").then_some(()));
    invoke(&previous);
    wait_for("back to the first match", || {
        (count() == "1 of 3").then_some(())
    });
    invoke(&previous);
    wait_for("wrap to the last match", || {
        (count() == "3 of 3").then_some(())
    });

    unsafe { value.SetValue(&BSTR::from("omega")) }.expect("SetValue");
    assert_eq!(count(), "", "typing clears the count, as in .NET");
    invoke(&next);
    wait_for("no matches", || (count() == "No matches").then_some(()));

    shell.activate();
    send_chord_to(shell.shell.id(), &[VK_ESCAPE]);
    wait_for("the hidden find bar", || {
        by_control_id(&shell.automation, &shell.root, FIND_EDIT_ID)
            .is_none_or(|edit| {
                unsafe { edit.CurrentIsOffscreen() }.is_ok_and(|offscreen| offscreen.as_bool())
            })
            .then_some(())
    });
}

/// The toolbar's find toggle (`ID_FIND`) and the bar's close button.
const FIND_TOGGLE_ID: u32 = 2008;
const FIND_CLOSE_ID: u32 = 1008;

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and an interactive desktop"]
fn the_toolbar_opens_the_find_bar_and_its_close_button_closes_it() {
    let shell = launch_two_documents(&unique_instance_key());
    let visible = |id: u32| {
        by_control_id(&shell.automation, &shell.root, id).is_some_and(|element| {
            unsafe { element.CurrentIsOffscreen() }.is_ok_and(|offscreen| !offscreen.as_bool())
        })
    };
    assert!(!visible(FIND_EDIT_ID), "the find bar starts hidden");
    let toggle =
        by_control_id(&shell.automation, &shell.root, FIND_TOGGLE_ID).expect("find toggle");
    assert_eq!(name(&toggle), "Find in document");
    invoke(&toggle);
    wait_for("the visible find bar", || {
        visible(FIND_EDIT_ID).then_some(())
    });
    let close = by_control_id(&shell.automation, &shell.root, FIND_CLOSE_ID).expect("close");
    assert_eq!(name(&close), "Close find bar");
    invoke(&close);
    wait_for("the hidden find bar", || {
        (!visible(FIND_EDIT_ID)).then_some(())
    });
}

// ---------------------------------------------------------------------------
// 4. Drag reorder (synthetic mouse input)
// ---------------------------------------------------------------------------

fn mouse(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

/// Sends one mouse event, only while `process_id` owns the foreground; the
/// check runs immediately before each `SendInput`.
fn guarded_send(process_id: u32, flags: MOUSE_EVENT_FLAGS) {
    assert_eq!(
        foreground_owner(),
        process_id,
        "shell must be foreground before mouse input"
    );
    let input = [mouse(flags)];
    let sent = unsafe { SendInput(&input, std::mem::size_of::<INPUT>() as i32) };
    assert_eq!(sent, 1, "SendInput");
}

/// Moves the cursor (no input event), only while the shell is foreground.
fn guarded_move(process_id: u32, x: i32, y: i32) {
    assert_eq!(
        foreground_owner(),
        process_id,
        "shell must be foreground before moving the cursor"
    );
    unsafe { SetCursorPos(x, y) }.expect("SetCursorPos");
}

/// Releases the left button when dropped, also after a failed assertion.
/// The button-up is always sent: a stuck button is worse than a stray
/// button-up, which types and clicks nothing.
struct LeftButtonGuard;

impl Drop for LeftButtonGuard {
    fn drop(&mut self) {
        let input = [mouse(MOUSEEVENTF_LEFTUP)];
        let _ = unsafe { SendInput(&input, std::mem::size_of::<INPUT>() as i32) };
    }
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn dragging_a_tab_reorders_it_and_raises_children_reordered() {
    let shell = launch_two_documents(&unique_instance_key());
    let names = shell.tab_names();
    let (events, _registration) = record_structure(&shell);
    let tabs = shell.tabs();
    let rect = |tab: &IUIAutomationElement| unsafe { tab.CurrentBoundingRectangle() }.unwrap();
    let (first, second) = (rect(&tabs[0]), rect(&tabs[1]));
    // Press on the first tab's body (left quarter, away from the close
    // button), then drag past the middle of the second tab.
    let y = (first.top + first.bottom) / 2;
    let start = first.left + (first.right - first.left) / 4;
    let end = second.left + (second.right - second.left) / 2;

    // The second document starts selected; the press selects the first.
    assert_eq!(shell.selected_name().as_deref(), Some(names[1].as_str()));
    shell.activate();
    let id = shell.shell.id();
    guarded_move(id, start, y);
    guarded_send(id, MOUSEEVENTF_LEFTDOWN);
    let button = LeftButtonGuard;
    // The shell reports the press: it selects the pressed tab.
    wait_for("the press to select the first tab", || {
        (shell.selected_name().as_deref() == Some(names[0].as_str())).then_some(())
    });
    // Move in small steps, each delivered as input, until the shell has
    // moved the tab live; past the end, keep nudging the pointer.
    let swapped = || shell.tab_names() == [names[1].clone(), names[0].clone()];
    let deadline = Instant::now() + TIMEOUT;
    let mut step = 0;
    while !swapped() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the swapped tab order (order {:?})",
            shell.tab_names()
        );
        step += 1;
        let x = if step <= 20 {
            start + (end - start) * step / 20
        } else {
            end + (step - 20) % 3 - 1
        };
        guarded_move(id, x, y);
        // SetCursorPos moves the cursor; a zero move delivers WM_MOUSEMOVE.
        guarded_send(id, MOUSEEVENTF_MOVE);
        thread::sleep(Duration::from_millis(50));
    }
    // Normal release behind the guard; the drop guard covers failures.
    guarded_send(id, MOUSEEVENTF_LEFTUP);
    std::mem::forget(button);
    wait_for("ChildrenReordered", || {
        events
            .lock()
            .ok()?
            .iter()
            .any(|event| event.change == StructureChangeType_ChildrenReordered)
            .then_some(())
    });
}
