#![cfg(windows)]

//! Native gate for link navigation between documents (`uia-native-links`):
//! a two-file wiki repository, links followed through UIA Invoke on the
//! WebView2 page's hyperlinks (Chromium's accessibility default action
//! raises the same click as a mouse), anchors, back and forward, the mouse
//! back button (`WM_APPCOMMAND`), the hovered-link status text, and Enter
//! on a focused link. It prints click-to-first-render timings.
//!
//! Ignored by default: it needs `MARKNEXIA_SHELL_EXE` and an unlocked
//! interactive desktop. Every launch uses an isolated settings file,
//! instance key and WebView2 profile (`support/shell.rs`). Only the Enter
//! test sends a key, through `send_chord_to`, which re-checks that the
//! launched shell owns the foreground first.

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Variant::VARIANT,
        },
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
                TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId,
                UIA_CONTROLTYPE_ID, UIA_ControlTypePropertyId, UIA_HyperlinkControlTypeId,
                UIA_InvokePatternId, UIA_NamePropertyId, UIA_TabItemControlTypeId,
                UIA_TextControlTypeId,
            },
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
            Input::KeyboardAndMouse::VK_RETURN,
            WindowsAndMessaging::{PostMessageW, WM_APPCOMMAND},
        },
    },
    core::{BSTR, Interface},
};

#[path = "support/shell.rs"]
mod shell_support;

use shell_support::{
    ShellProcess, activate_attempt, find_shell_window, foreground_owner, send_chord_to,
};

const TIMEOUT: Duration = Duration::from_secs(30);

/// A wiki-style repository: `README.md` at the root (a `.git` folder marks
/// it) links to `00-overview/product-overview.md`, which links back to an
/// anchor in the README.
struct Wiki {
    root: PathBuf,
}

impl Wiki {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let root =
            std::env::temp_dir().join(format!("marknexia-links-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&root).expect("wiki folder");
        let root = std::path::absolute(root).expect("absolute wiki folder");
        std::fs::create_dir(root.join(".git")).expect(".git marker");
        std::fs::create_dir(root.join("00-overview")).expect("00-overview");
        let filler: String = (1..=80)
            .map(|line| format!("Filler paragraph {line} keeps the details far below.\n\n"))
            .collect();
        std::fs::write(
            root.join("README.md"),
            format!(
                "# Project docs home\n\n\
                 - [Product overview](00-overview/product-overview.md)\n\
                 - [Jump to details](#details-section)\n\
                 - [Missing page](00-overview/missing.md)\n\
                 - [Escape the repository](../outside.md)\n\n\
                 | Table | Link |\n| --- | --- |\n| row | [Table link](00-overview/Product-Overview.MD) |\n\n\
                 ### [Heading link](00-overview/product-overview.md)\n\n\
                 [![Image link](pictures/pic.png)](00-overview%2Fproduct-overview.md)\n\n\
                 {filler}\
                 ## Details section\n\n\
                 [Open overview from details](./00-overview/product-overview.md#goals-heading)\n\n\
                 {filler}"
            ),
        )
        .expect("README.md");
        std::fs::write(
            root.join("00-overview").join("product-overview.md"),
            format!(
                "# Product overview page\n\n[Back home](../README.md#details-section)\n\n\
                 {filler}## Goals heading\n\nThe goals.\n"
            ),
        )
        .expect("product-overview.md");
        Self { root }
    }

    fn readme(&self) -> PathBuf {
        self.root.join("README.md")
    }
}

impl Drop for Wiki {
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

struct Launched {
    // Drop order: UIA objects, then the apartment, then the process.
    root: IUIAutomationElement,
    automation: IUIAutomation,
    _apartment: Apartment,
    hwnd: HWND,
    shell: ShellProcess,
}

fn launch(wiki: &Wiki) -> Launched {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let readme = wiki.readme();
    let shell = ShellProcess::launch(&executable, &[readme.as_os_str()]);
    let deadline = Instant::now() + TIMEOUT;
    let hwnd = loop {
        if let Some(hwnd) = find_shell_window(shell.id()) {
            break hwnd;
        }
        assert!(Instant::now() < deadline, "the shell window");
        thread::sleep(Duration::from_millis(100));
    };
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

impl Launched {
    fn find(&self, kind: UIA_CONTROLTYPE_ID, wanted: &str) -> Option<IUIAutomationElement> {
        let by_name = unsafe {
            self.automation
                .CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(wanted)))
        }
        .ok()?;
        let by_kind = unsafe {
            self.automation
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(kind.0))
        }
        .ok()?;
        let condition = unsafe { self.automation.CreateAndCondition(&by_name, &by_kind) }.ok()?;
        unsafe { self.root.FindFirst(TreeScope_Descendants, &condition) }.ok()
    }

    fn find_id(&self, id: &str) -> Option<IUIAutomationElement> {
        let condition = unsafe {
            self.automation
                .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(BSTR::from(id)))
        }
        .ok()?;
        unsafe { self.root.FindFirst(TreeScope_Descendants, &condition) }.ok()
    }

    fn until<T>(&self, what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(value) = probe() {
                return value;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn link(&self, text: &str) -> IUIAutomationElement {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(link) = self.find(UIA_HyperlinkControlTypeId, text) {
                return link;
            }
            if Instant::now() >= deadline {
                let names = self.names(UIA_HyperlinkControlTypeId);
                panic!("timed out waiting for the link {text:?}; links: {names:?}");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// The names of every element of `kind` (failure diagnostics).
    fn names(&self, kind: UIA_CONTROLTYPE_ID) -> Vec<String> {
        let Ok(condition) = (unsafe {
            self.automation
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(kind.0))
        }) else {
            return Vec::new();
        };
        let Ok(found) = (unsafe { self.root.FindAll(TreeScope_Descendants, &condition) }) else {
            return Vec::new();
        };
        (0..unsafe { found.Length() }.unwrap_or(0))
            .filter_map(|index| unsafe { found.GetElement(index) }.ok())
            .map(|element| name(&element))
            .collect()
    }

    /// The page heading (or any page text) named `text`.
    fn text(&self, text: &str) -> Option<IUIAutomationElement> {
        self.find(UIA_TextControlTypeId, text)
    }

    fn tab(&self, title: &str) -> Option<IUIAutomationElement> {
        self.find(UIA_TabItemControlTypeId, title)
    }

    fn status(&self) -> String {
        self.find_id("1005")
            .map(|status| name(&status))
            .unwrap_or_default()
    }

    fn diagnostic(&self) -> String {
        self.find_id("2020")
            .map(|text| name(&text))
            .unwrap_or_default()
    }

    fn button(&self, id: &str, wanted: &str) -> IUIAutomationElement {
        self.until(&format!("the {wanted} button"), || {
            self.find_id(id)
                .filter(|element| {
                    unsafe { element.CurrentControlType() }
                        .is_ok_and(|kind| kind == UIA_ButtonControlTypeId)
                })
                .filter(|element| name(element) == wanted)
        })
    }

    /// Invokes `link` and waits until the page shows `heading`; returns the
    /// click-to-first-render time (UIA polling adds a few milliseconds).
    fn follow(&self, link: &IUIAutomationElement, heading: &str) -> Duration {
        let started = Instant::now();
        invoke(link);
        self.until(&format!("the heading {heading:?}"), || self.text(heading));
        started.elapsed()
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

    /// The mouse back button over the shell (`WM_APPCOMMAND`,
    /// `APPCOMMAND_BROWSER_BACKWARD` from the mouse).
    fn mouse_back(&self) {
        const APPCOMMAND_BROWSER_BACKWARD: isize = 1;
        const FAPPCOMMAND_MOUSE: isize = 0x8000;
        let lparam = (APPCOMMAND_BROWSER_BACKWARD | FAPPCOMMAND_MOUSE) << 16;
        // SAFETY: a plain post to the launched shell's window; no pointers.
        unsafe {
            PostMessageW(
                Some(self.hwnd),
                WM_APPCOMMAND,
                WPARAM(self.hwnd.0 as usize),
                LPARAM(lparam),
            )
        }
        .expect("post WM_APPCOMMAND");
    }
}

const HOME: &str = "Project docs home";
const OVERVIEW: &str = "Product overview page";
const DETAILS: &str = "Details section";
const GOALS: &str = "Goals heading";

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn relative_links_anchors_and_back_navigate_in_the_same_tab() {
    let wiki = Wiki::new();
    let shell = launch(&wiki);
    shell.until("the README tab", || shell.tab("README.md"));
    shell.until("the README heading", || shell.text(HOME));
    let back = shell.button("2002", "Back");
    let forward = shell.button("2003", "Forward");
    assert!(!is_enabled(&back));

    // Focus shows the resolved target in the status bar, as hovering does.
    let overview_link = shell.link("Product overview");
    unsafe { overview_link.SetFocus() }.expect("focus the link");
    shell.until("the hovered link in the status bar", || {
        (shell.status() == "00-overview/product-overview.md").then_some(())
    });

    // A relative link: same tab, renamed, new document, history recorded.
    let cold = shell.follow(&overview_link, OVERVIEW);
    shell.until("the tab renamed", || shell.tab("product-overview.md"));
    assert!(
        shell.tab("README.md").is_none(),
        "the same tab shows the target"
    );
    shell.until("Back enabled", || is_enabled(&back).then_some(()));
    eprintln!("click-to-first-render, cold: {} ms", cold.as_millis());

    // Back: the README again.
    let started = Instant::now();
    invoke(&back);
    shell.until("the README after Back", || shell.text(HOME));
    eprintln!("back-to-first-render: {} ms", started.elapsed().as_millis());
    shell.until("the tab named README.md", || shell.tab("README.md"));
    shell.until("Forward enabled", || is_enabled(&forward).then_some(()));

    // The same link again: the controller and renderer are warm.
    let warm = shell.follow(&shell.link("Product overview"), OVERVIEW);
    eprintln!("click-to-first-render, warm: {} ms", warm.as_millis());
    // Once more without UIA polling while the page loads, so the shell's
    // own timeline is not slowed by cross-process tree queries.
    invoke(&back);
    shell.until("the README after Back", || shell.text(HOME));
    invoke(&shell.link("Product overview"));
    thread::sleep(Duration::from_millis(1500));
    shell.until("the overview (quiet)", || shell.text(OVERVIEW));

    // Cross-document link with an anchor: README at the details section.
    let home = shell.follow(&shell.link("Back home"), HOME);
    eprintln!("cross-document anchor link: {} ms", home.as_millis());
    let details = shell.until("the details heading", || shell.text(DETAILS));
    shell.until("the details heading scrolled into view", || {
        is_onscreen(&details).then_some(())
    });

    // From the details section to the overview's goals, then the mouse
    // back button: the README returns at the details section (its scroll
    // offset is restored, LNK-3).
    let goals_time = shell.follow(&shell.link("Open overview from details"), GOALS);
    eprintln!(
        "cross-document anchor link (goals): {} ms",
        goals_time.as_millis()
    );
    shell.mouse_back();
    shell.until("the README after the mouse back button", || {
        shell.text(HOME)
    });
    let details = shell.until("the details heading", || shell.text(DETAILS));
    shell.until("the scroll offset restored", || {
        is_onscreen(&details).then_some(())
    });
    shell.until("Forward enabled after the mouse back button", || {
        is_enabled(&forward).then_some(())
    });
    print_timings(&shell);
}

/// The shell's own timeline per navigation (`MARKNEXIA_LINK_TIMING_LOG`):
/// milliseconds from the click (or Back) to the link's resolution, the
/// render start, the document handed to the controller and its load.
fn print_timings(shell: &Launched) {
    thread::sleep(Duration::from_millis(300));
    let log = std::fs::read_to_string(shell.shell.link_timing_log_path()).unwrap_or_default();
    for line in log.lines() {
        eprintln!("shell timeline: {line}");
    }
    assert!(!log.is_empty(), "the shell recorded its navigation timings");
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn anchor_links_scroll_and_record_history_and_bad_links_report() {
    let wiki = Wiki::new();
    let shell = launch(&wiki);
    shell.until("the README heading", || shell.text(HOME));
    let back = shell.button("2002", "Back");
    let details = shell.until("the details heading", || shell.text(DETAILS));
    assert!(!is_onscreen(&details), "the details start below the fold");

    invoke(&shell.link("Jump to details"));
    shell.until("the details heading scrolled into view", || {
        is_onscreen(&details).then_some(())
    });
    shell.until("Back enabled after the anchor", || {
        is_enabled(&back).then_some(())
    });
    assert!(
        shell.tab("README.md").is_some(),
        "an anchor stays in the document"
    );

    invoke(&back);
    shell.until("the top again after Back", || {
        (!is_onscreen(&details)).then_some(())
    });

    // The .NET messages for a missing target and an escaping link.
    invoke(&shell.link("Missing page"));
    shell.until("the missing-target warning", || {
        shell
            .diagnostic()
            .contains("Target file not found:")
            .then_some(())
    });
    invoke(&shell.link("Escape the repository"));
    shell.until("the sandbox error", || {
        shell
            .diagnostic()
            .contains("Access blocked: Relative path escapes repository root sandbox.")
            .then_some(())
    });
    assert!(shell.tab("README.md").is_some(), "nothing opened");

    // Links inside a table cell, a heading and around an image follow the
    // same way (the image link's path is percent-encoded, and the table
    // link's case differs from the file's).
    // The image is missing, so the bridge shows its fallback text.
    for text in [
        "Table link",
        "Heading link",
        "Image unavailable: Image link",
    ] {
        let link = shell.link(text);
        shell.follow(&link, OVERVIEW);
        shell.until("the tab renamed", || {
            shell
                .tab("product-overview.md")
                .or_else(|| shell.tab("Product-Overview.MD"))
        });
        invoke(&back);
        shell.until("the README after Back", || shell.text(HOME));
    }

    // A cross-document link with an anchor scrolls the new document.
    invoke(&shell.link("Open overview from details"));
    let goals = shell.until("the goals heading", || shell.text(GOALS));
    shell.until("the goals heading scrolled into view", || {
        is_onscreen(&goals).then_some(())
    });
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE, an interactive desktop and the foreground"]
fn enter_on_a_focused_link_follows_it() {
    let wiki = Wiki::new();
    let shell = launch(&wiki);
    shell.until("the README heading", || shell.text(HOME));
    let link = shell.link("Product overview");
    assert!(
        shell.activate(),
        "the shell must own the foreground to send Enter"
    );
    unsafe { link.SetFocus() }.expect("focus the link");
    shell.until("the focused link in the status bar", || {
        (shell.status() == "00-overview/product-overview.md").then_some(())
    });
    send_chord_to(shell.shell.id(), &[VK_RETURN]);
    shell.until("the overview after Enter", || shell.text(OVERVIEW));
    shell.until("the tab renamed", || shell.tab("product-overview.md"));
}
