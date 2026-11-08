#![cfg(windows)]

//! Native gate `uia-native-keyboard`: a keyboard reachability audit of the
//! shell chrome.
//!
//! The test walks the chrome with Tab and with Shift+Tab and reads the UIA
//! focus after every key. It asserts that:
//!
//! 1. every interactive chrome control is reachable by keyboard: the
//!    toolbar buttons, the theme combo box, the document tabs, the sidebar
//!    view selector, the outline list or the repository tree, the sidebar
//!    resize handle and the find bar; and every enabled, on-screen,
//!    keyboard-focusable chrome element that UIA reports is in the walk;
//! 2. every stop has a non-empty UIA Name and a ControlType that fits the
//!    control (a status text or another non-interactive stop fails);
//! 3. every focused element reports `HasKeyboardFocus`;
//! 4. Tab returns to the first control (no focus trap), and Shift+Tab walks
//!    the same stops in reverse order.
//!
//! The tab order is printed. Keys go only through `send_chord_to`, which
//! re-checks that the launched shell owns the foreground. No system setting
//! is changed. Every launch uses an isolated settings file, instance key and
//! WebView2 profile (`support/shell.rs`).

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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
                CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Descendants,
                UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID,
                UIA_ComboBoxControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
                UIA_ListControlTypeId, UIA_ListItemControlTypeId, UIA_MenuBarControlTypeId,
                UIA_MenuItemControlTypeId, UIA_PaneControlTypeId, UIA_SeparatorControlTypeId,
                UIA_TabControlTypeId, UIA_TabItemControlTypeId, UIA_ThumbControlTypeId,
                UIA_TitleBarControlTypeId, UIA_TreeControlTypeId, UIA_TreeItemControlTypeId,
            },
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
            Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_CONTROL, VK_SHIFT, VK_TAB},
        },
    },
    core::BSTR,
};

#[path = "support/shell.rs"]
mod shell_support;

use shell_support::{
    ShellProcess, activate_attempt, find_shell_window, foreground_owner, send_chord_to,
};

const TIMEOUT: Duration = Duration::from_secs(30);
/// More stops than the chrome has: a longer walk means focus never
/// returned to the first control.
const MAX_STOPS: usize = 40;
const VK_F: VIRTUAL_KEY = VIRTUAL_KEY(0x46);

/// A repository (a `.git` folder marks it) with a README that has headings
/// for the outline and a subfolder for the tree.
struct Repository {
    root: PathBuf,
}

impl Repository {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let root =
            std::env::temp_dir().join(format!("marknexia-keyboard-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&root).expect("repository folder");
        let root = std::path::absolute(root).expect("absolute repository folder");
        std::fs::create_dir(root.join(".git")).expect(".git marker");
        std::fs::create_dir(root.join("docs")).expect("docs folder");
        std::fs::write(
            root.join("README.md"),
            "# Keyboard audit\n\nIntro text.\n\n## Second heading\n\nMore text.\n\n\
             ## Third heading\n\nThe end.\n",
        )
        .expect("README.md");
        std::fs::write(root.join("docs").join("guide.md"), "# Guide\n").expect("guide.md");
        Self { root }
    }

    fn readme(&self) -> PathBuf {
        self.root.join("README.md")
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

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

fn automation_id(element: &IUIAutomationElement) -> String {
    unsafe { element.CurrentAutomationId() }
        .map(|id| id.to_string())
        .unwrap_or_default()
}

fn kind(element: &IUIAutomationElement) -> UIA_CONTROLTYPE_ID {
    unsafe { element.CurrentControlType() }.unwrap_or_default()
}

fn kind_name(kind: UIA_CONTROLTYPE_ID) -> String {
    let known = [
        (UIA_ButtonControlTypeId, "Button"),
        (UIA_ComboBoxControlTypeId, "ComboBox"),
        (UIA_EditControlTypeId, "Edit"),
        (UIA_ListControlTypeId, "List"),
        (UIA_ListItemControlTypeId, "ListItem"),
        (UIA_PaneControlTypeId, "Pane"),
        (UIA_TabControlTypeId, "Tab"),
        (UIA_TabItemControlTypeId, "TabItem"),
        (UIA_TreeControlTypeId, "Tree"),
        (UIA_TreeItemControlTypeId, "TreeItem"),
        (UIA_DocumentControlTypeId, "Document"),
        (UIA_ThumbControlTypeId, "Thumb"),
        (UIA_SeparatorControlTypeId, "Separator"),
    ];
    known.iter().find(|(id, _)| *id == kind).map_or_else(
        || format!("ControlType {}", kind.0),
        |(_, name)| (*name).to_owned(),
    )
}

/// The ControlTypes that fit the chrome control with this automation id
/// (its Win32 control ID). `None`: not an expected tab stop.
fn expected_kinds(id: &str) -> Option<&'static [UIA_CONTROLTYPE_ID]> {
    Some(match id {
        // Toolbar, open file, sidebar row actions, find bar, diagnostics.
        "2001" | "2002" | "2003" | "2004" | "2005" | "2008" | "1001" | "2018" | "2019" | "1006"
        | "1007" | "1008" | "2021" => &[UIA_ButtonControlTypeId],
        "2007" => &[UIA_ComboBoxControlTypeId],
        // The document tabs and the sidebar view selector: the tab control
        // or its selected item.
        "1002" | "1003" => &[UIA_TabControlTypeId, UIA_TabItemControlTypeId],
        // Outline and recent files.
        "2011" | "2013" => &[UIA_ListControlTypeId, UIA_ListItemControlTypeId],
        "2012" => &[UIA_TreeControlTypeId, UIA_TreeItemControlTypeId],
        "1004" => &[UIA_EditControlTypeId],
        // The WebView2 page (Chromium's root web area): Tab enters the
        // document after the last chrome control and leaves it again.
        "RootWebArea" => &[UIA_DocumentControlTypeId],
        // The resize handle is a plain window with the default proxy (a
        // named Pane); see compat/decisions/accessibility-gates.md (A11Y-2).
        "2014" => &[
            UIA_PaneControlTypeId,
            UIA_ThumbControlTypeId,
            UIA_SeparatorControlTypeId,
        ],
        _ => return None,
    })
}

/// One keyboard stop.
#[derive(Clone, Debug)]
struct Stop {
    /// The automation id of the focused element or its nearest ancestor
    /// that has one (the Win32 control ID of the chrome control).
    control: String,
    name: String,
    kind: UIA_CONTROLTYPE_ID,
    has_focus: bool,
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {:?} [{}] HasKeyboardFocus={}",
            kind_name(self.kind),
            self.name,
            self.control,
            self.has_focus
        )
    }
}

struct Launched {
    // Drop order: UIA objects, then the apartment, then the process.
    root: IUIAutomationElement,
    automation: IUIAutomation,
    _apartment: Apartment,
    hwnd: HWND,
    shell: ShellProcess,
}

fn launch(repository: &Repository, settings: &str) -> Launched {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let readme = repository.readme();
    let shell = ShellProcess::launch_with_settings(
        &executable,
        &[readme.as_os_str()],
        Some(settings.as_bytes()),
    );
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

impl Launched {
    fn element_by_id(&self, id: &str) -> Option<IUIAutomationElement> {
        let condition = unsafe {
            self.automation
                .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(BSTR::from(id)))
        }
        .ok()?;
        unsafe { self.root.FindFirst(TreeScope_Descendants, &condition) }.ok()
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

    /// The automation id of `element` or of its nearest ancestor below the
    /// shell window that has one.
    fn control_id(&self, element: &IUIAutomationElement) -> String {
        let Ok(walker) = (unsafe { self.automation.RawViewWalker() }) else {
            return String::new();
        };
        let mut current = element.clone();
        for _ in 0..8 {
            let id = automation_id(&current);
            if !id.is_empty() {
                return id;
            }
            match unsafe { walker.GetParentElement(&current) } {
                Ok(parent) => {
                    if unsafe { self.automation.CompareElements(&parent, &self.root) }
                        .is_ok_and(|same| same.as_bool())
                    {
                        break;
                    }
                    current = parent;
                }
                Err(_) => break,
            }
        }
        String::new()
    }

    fn focused(&self) -> Option<(IUIAutomationElement, Stop)> {
        let element = unsafe { self.automation.GetFocusedElement() }.ok()?;
        let stop = Stop {
            control: self.control_id(&element),
            name: name(&element),
            kind: kind(&element),
            has_focus: unsafe { element.CurrentHasKeyboardFocus() }
                .is_ok_and(|focus| focus.as_bool()),
        };
        Some((element, stop))
    }

    /// The focused element once it has stayed the same for 200 ms (at most
    /// two seconds): focus can pass through a host window on its way into
    /// the WebView2 document, and selection can follow focus.
    fn settled(
        &self,
        mut element: IUIAutomationElement,
        mut stop: Stop,
    ) -> (IUIAutomationElement, Stop) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(200));
            let Some((next, next_stop)) = self.focused() else {
                continue;
            };
            let same = unsafe { self.automation.CompareElements(&next, &element) }
                .is_ok_and(|same| same.as_bool());
            let unchanged = same && next_stop.has_focus == stop.has_focus;
            element = next;
            stop = next_stop;
            if unchanged {
                break;
            }
        }
        (element, stop)
    }

    /// Sends `keys` and waits until UIA reports a different focused element.
    /// `None` when focus did not move (a trap, or a single stop).
    fn press_and_read(
        &self,
        keys: &[VIRTUAL_KEY],
        previous: &IUIAutomationElement,
    ) -> Option<(IUIAutomationElement, Stop)> {
        send_chord_to(self.shell.id(), keys);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(60));
            if let Some((element, stop)) = self.focused() {
                let same = unsafe { self.automation.CompareElements(&element, previous) }
                    .is_ok_and(|same| same.as_bool());
                if !same {
                    return Some(self.settled(element, stop));
                }
            }
        }
        None
    }

    /// Focuses the control `start` through UIA, then presses `keys` until
    /// focus returns to it. Returns the stops in order, `start` first.
    fn walk(&self, start: &str, keys: &[VIRTUAL_KEY], label: &str) -> Vec<Stop> {
        let element = wait_for(&format!("the control {start}"), || {
            self.element_by_id(start)
        });
        unsafe { element.SetFocus() }.expect("UIA SetFocus on the first control");
        let (mut current, first) = wait_for(&format!("focus on {start}"), || {
            self.focused().filter(|(_, stop)| stop.control == start)
        });
        let mut stops = vec![first];
        loop {
            let Some((element, stop)) = self.press_and_read(keys, &current) else {
                panic!(
                    "{label}: focus did not move from {} (focus trap); stops so far: {}",
                    stops.last().expect("a stop"),
                    render(&stops)
                );
            };
            if stop.control == start {
                break;
            }
            stops.push(stop);
            assert!(
                stops.len() <= MAX_STOPS,
                "{label}: focus never returned to {start} within {MAX_STOPS} stops: {}",
                render(&stops)
            );
            current = element;
        }
        eprintln!("{label} tab order:\n{}", render(&stops));
        stops
    }

    /// Enabled, on-screen, keyboard-focusable chrome elements that own a
    /// child window of the shell (not the WebView2 document, the menu bar
    /// or the title bar), by automation id.
    fn focusable_chrome(&self) -> Vec<(String, String)> {
        let Ok(condition) = (unsafe { self.automation.CreateTrueCondition() }) else {
            return Vec::new();
        };
        let Ok(found) = (unsafe { self.root.FindAll(TreeScope_Descendants, &condition) }) else {
            return Vec::new();
        };
        (0..unsafe { found.Length() }.unwrap_or(0))
            .filter_map(|index| unsafe { found.GetElement(index) }.ok())
            .filter(|element| unsafe {
                element
                    .CurrentIsKeyboardFocusable()
                    .is_ok_and(|f| f.as_bool())
                    && element.CurrentIsEnabled().is_ok_and(|e| e.as_bool())
                    && element.CurrentIsOffscreen().is_ok_and(|o| !o.as_bool())
                    && element
                        .CurrentNativeWindowHandle()
                        .is_ok_and(|window| !window.is_invalid() && window != self.hwnd)
            })
            .filter(|element| {
                let class = unsafe { element.CurrentClassName() }
                    .map(|class| class.to_string())
                    .unwrap_or_default();
                let framework = unsafe { element.CurrentFrameworkId() }
                    .map(|id| id.to_string())
                    .unwrap_or_default();
                !class.starts_with("Chrome_")
                    && framework != "Chrome"
                    && ![
                        UIA_DocumentControlTypeId,
                        UIA_MenuBarControlTypeId,
                        UIA_MenuItemControlTypeId,
                        UIA_TitleBarControlTypeId,
                    ]
                    .contains(&kind(element))
            })
            .map(|element| (automation_id(&element), name(&element)))
            .collect()
    }

    fn is_enabled(&self, id: &str) -> bool {
        self.element_by_id(id).is_some_and(|element| unsafe {
            element.CurrentIsEnabled().is_ok_and(|e| e.as_bool())
                && element.CurrentIsOffscreen().is_ok_and(|o| !o.as_bool())
        })
    }
}

fn render(stops: &[Stop]) -> String {
    stops
        .iter()
        .enumerate()
        .map(|(index, stop)| format!("  {:>2}. {stop}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Names, ControlTypes and keyboard focus of every stop.
fn assert_stops_accessible(stops: &[Stop], label: &str) {
    for stop in stops {
        assert!(
            !stop.name.trim().is_empty(),
            "{label}: unnamed keyboard stop {stop}"
        );
        let Some(kinds) = expected_kinds(&stop.control) else {
            panic!(
                "{label}: unexpected keyboard stop {stop}: not an interactive chrome control \
                 (order:\n{})",
                render(stops)
            );
        };
        assert!(
            kinds.contains(&stop.kind),
            "{label}: {stop} has ControlType {}; expected one of {:?}",
            kind_name(stop.kind),
            kinds
                .iter()
                .map(|kind| kind_name(*kind))
                .collect::<Vec<_>>()
        );
        assert!(
            stop.has_focus,
            "{label}: {stop} is focused but does not report HasKeyboardFocus"
        );
    }
}

/// The required controls are in the walk, and so is every focusable
/// chrome element UIA reports.
fn assert_reachable(shell: &Launched, stops: &[Stop], required: &[&str], label: &str) {
    let reached: Vec<&str> = stops.iter().map(|stop| stop.control.as_str()).collect();
    for id in required {
        assert!(
            reached.contains(id),
            "{label}: control {id} is not reachable with the keyboard; order:\n{}",
            render(stops)
        );
    }
    for (id, element_name) in shell.focusable_chrome() {
        assert!(
            !id.is_empty() && reached.contains(&id.as_str()),
            "{label}: focusable chrome element {element_name:?} [{id}] is not reachable \
             with Tab; order:\n{}",
            render(stops)
        );
    }
}

/// Shift+Tab visits the Tab stops in reverse order.
fn assert_reverse(forward: &[Stop], backward: &[Stop], label: &str) {
    let mut expected: Vec<&str> = forward.iter().map(|stop| stop.control.as_str()).collect();
    expected[1..].reverse();
    let actual: Vec<&str> = backward.iter().map(|stop| stop.control.as_str()).collect();
    assert_eq!(
        actual, expected,
        "{label}: Shift+Tab order is not the reverse of the Tab order"
    );
}

/// The toolbar controls every view has; Back and Forward only when enabled.
fn toolbar_required(shell: &Launched) -> Vec<&'static str> {
    let mut required = vec![
        "2001", "2004", "1001", "2005", "2008", "2007", "1002", "1003",
    ];
    for optional in ["2002", "2003"] {
        if shell.is_enabled(optional) {
            required.push(optional);
        }
    }
    required
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and an unlocked interactive desktop"]
fn keyboard_reaches_every_control_in_the_outline_view_and_find_bar() {
    let repository = Repository::new();
    let shell = launch(&repository, "{\"SidebarMode\": 0, \"IsSidebarOpen\": true}");
    let outline = wait_for("the document outline", || {
        shell
            .element_by_id("2011")
            .filter(|_| shell.is_enabled("2011"))
    });
    wait_for("the outline's headings", || {
        let condition = unsafe { shell.automation.CreateTrueCondition() }.ok()?;
        let items = unsafe { outline.FindAll(TreeScope_Descendants, &condition) }.ok()?;
        (unsafe { items.Length() }.ok()? >= 3).then_some(())
    });
    assert!(
        shell.activate(),
        "the shell must own the foreground to send Tab"
    );

    let forward = shell.walk("2001", &[VK_TAB], "outline view, Tab");
    assert_stops_accessible(&forward, "outline view");
    let mut required = toolbar_required(&shell);
    required.extend(["2011", "2014"]);
    assert_reachable(&shell, &forward, &required, "outline view");
    let backward = shell.walk("2001", &[VK_SHIFT, VK_TAB], "outline view, Shift+Tab");
    assert_reverse(&forward, &backward, "outline view");

    // The find bar: Ctrl+F shows it and focuses the box.
    send_chord_to(shell.shell.id(), &[VK_CONTROL, VK_F]);
    wait_for("the find box to take focus", || {
        shell.focused().filter(|(_, stop)| stop.control == "1004")
    });
    let find = shell.walk("1004", &[VK_TAB], "find bar open, Tab");
    assert_stops_accessible(&find, "find bar open");
    let mut required = toolbar_required(&shell);
    required.extend(["1004", "1008"]);
    for optional in ["1006", "1007"] {
        if shell.is_enabled(optional) {
            required.push(optional);
        }
    }
    assert_reachable(&shell, &find, &required, "find bar open");
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and an unlocked interactive desktop"]
fn keyboard_reaches_every_control_in_the_repository_view() {
    let repository = Repository::new();
    let settings = format!(
        "{{\"RepositoryRoot\": {}, \"SidebarMode\": 1, \"IsSidebarOpen\": true}}",
        serde_json::to_string(&repository.root.to_string_lossy()).expect("JSON string")
    );
    let shell = launch(&repository, &settings);
    wait_for("the repository tree", || {
        shell
            .element_by_id("2012")
            .filter(|_| shell.is_enabled("2012"))
    });
    assert!(
        shell.activate(),
        "the shell must own the foreground to send Tab"
    );

    let forward = shell.walk("2001", &[VK_TAB], "repository view, Tab");
    assert_stops_accessible(&forward, "repository view");
    let mut required = toolbar_required(&shell);
    required.extend(["2012", "2014"]);
    assert_reachable(&shell, &forward, &required, "repository view");
    let backward = shell.walk("2001", &[VK_SHIFT, VK_TAB], "repository view, Shift+Tab");
    assert_reverse(&forward, &backward, "repository view");
}
