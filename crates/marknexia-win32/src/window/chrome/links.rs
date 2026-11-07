//! Link navigation between documents, ported from the .NET `MainWindow`:
//! `CoreWebView2_WebMessageReceived` and `HandleLinkClick`, then
//! `OpenDocumentInTabAsync` with `reuseActiveTab: true`
//! (`ReplaceActiveDocumentAsync`), `ApplyPendingLocationAsync`, and
//! `Back_Click` / `Forward_Click` across documents. Differences from .NET:
//! `compat/decisions/link-navigation.md` (LNK-1 to LNK-8).
//!
//! The page's link adapter (`marknexia-webview/src/link_adapter.js`) posts
//! typed, identity-bound messages. Every message is checked against the
//! tab's current document (tab and epoch) before it does anything. The
//! link is resolved by `marknexia-navigation` on a worker thread (repository
//! root detection and file probes touch the file system); the result is
//! applied on the UI thread, with no `AppState` borrow held across a
//! WebView2 call, a control message or a render start.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use marknexia_core::contracts::DiagnosticSeverity;
use marknexia_files::repository::detect_repository_root;
use marknexia_navigation::{FileProbe, NavigationOutcome, ResolutionContext};
use marknexia_webview::{host::document_origin, protocol::PageToHost};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::{WM_APPCOMMAND, WM_XBUTTONUP},
};

use super::{
    AppState, ChromeAction, ChromeCommand, TabLocation, WorkerResult, active_tab, queue_action,
    restore_session, same_document_path, settings_bridge, start_render, state_of, sync_ui,
    take_session,
};
use crate::{
    chrome_model::{LinkAction, fallback_link_href, link_action, link_status_text, reveal_allowed},
    documents::{check_document_path, probe_document_file, tab_title},
};

/// Mouse back and forward can arrive twice for one press (the page's
/// `mouseup` and the shell's `WM_APPCOMMAND`); a second step this soon
/// after the first is the same press.
const MOUSE_STEP_DEDUPE: Duration = Duration::from_millis(250);

thread_local! {
    static LAST_MOUSE_STEP: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Per tab: when the navigation started, what started it, and the
    /// steps so far (`MARKNEXIA_LINK_TIMING_LOG`).
    static TIMING: RefCell<std::collections::BTreeMap<u64, (Instant, &'static str, String)>> =
        const { RefCell::new(std::collections::BTreeMap::new()) };
}

/// Native tests record each navigation's timeline, one line per document
/// shown: what started it and the milliseconds to each step (`resolved`,
/// `render`, `replaced`, `loaded`). Honored only for an absolute path that
/// passes `GatedPath`, like `MARKNEXIA_REVEAL_LOG`; written by a short
/// worker, never on the UI thread.
const TIMING_LOG_VARIABLE: &str = "MARKNEXIA_LINK_TIMING_LOG";

fn timing_log() -> Option<PathBuf> {
    std::env::var_os(TIMING_LOG_VARIABLE)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .filter(|path| marknexia_files::path::GatedPath::new(&path.to_string_lossy()).is_ok())
}

/// Starts `tab`'s navigation timeline.
fn timing_begin(tab: u64, cause: &'static str) {
    if timing_log().is_some() {
        TIMING.with(|timing| {
            if let Ok(mut timing) = timing.try_borrow_mut() {
                timing.insert(tab, (Instant::now(), cause, String::new()));
            }
        });
    }
}

/// Records `step` in `tab`'s timeline, if one is running.
pub(in crate::window) fn timing_step(tab: u64, step: &str) {
    TIMING.with(|timing| {
        if let Ok(mut timing) = timing.try_borrow_mut()
            && let Some((started, _, steps)) = timing.get_mut(&tab)
        {
            steps.push_str(&format!(" {step}={}", started.elapsed().as_millis()));
        }
    });
}

/// `tab`'s new document loaded: the timeline ends and is written.
pub(in crate::window) fn timing_loaded(tab: u64) {
    let Some(log) = timing_log() else {
        return;
    };
    let finished = TIMING.with(|timing| {
        timing
            .try_borrow_mut()
            .ok()
            .and_then(|mut timing| timing.remove(&tab))
    });
    if let Some((started, cause, steps)) = finished {
        let line = format!("{cause}{steps} loaded={}\n", started.elapsed().as_millis());
        let _ = std::thread::Builder::new()
            .name("marknexia-link-timing".to_owned())
            .spawn(move || {
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log)
                {
                    let _ = file.write_all(line.as_bytes());
                }
            });
    }
}

/// `APPCOMMAND_BROWSER_BACKWARD` and `APPCOMMAND_BROWSER_FORWARD`.
const APPCOMMAND_BROWSER_BACKWARD: u32 = 1;
const APPCOMMAND_BROWSER_FORWARD: u32 = 2;
/// `XBUTTON1` (back) in the high word of `WM_XBUTTONUP`'s wParam.
const XBUTTON1: usize = 1;

/// The resolver's file probe: [`probe_document_file`], confined to the
/// repository root when the sandbox is enforced.
struct NativeProbe {
    root: Option<PathBuf>,
}

impl FileProbe for NativeProbe {
    fn file_exists(&self, path: &str, probes: &mut u64) -> bool {
        *probes += 1;
        probe_document_file(
            &PathBuf::from(path.replace('/', "\\")),
            self.root.as_deref(),
        )
    }
}

/// What a link resolution needs, copied out of the state for the worker.
struct LinkRequest {
    href: String,
    current_file: PathBuf,
    active_root: Option<PathBuf>,
    allow_external_links: bool,
    enforce_repository_sandbox: bool,
}

/// Runs on a worker: the document's repository root (.NET
/// `ResolveRepositoryRootForDocument`: the open repository when it contains
/// the document, else the detected one), then the resolver with the native
/// probe.
fn resolve_link(request: LinkRequest) -> NavigationOutcome {
    let current = request.current_file.to_string_lossy().into_owned();
    let root = request
        .active_root
        .filter(|root| reveal_allowed(&request.current_file, Some(root)))
        .or_else(|| {
            (!current.is_empty())
                .then(|| detect_repository_root(&current).ok().flatten())
                .flatten()
        });
    let context = ResolutionContext {
        current_file: current,
        repository_root: root
            .as_ref()
            .map(|root| root.to_string_lossy().into_owned()),
        allow_external_links: request.allow_external_links,
        enforce_repository_sandbox: request.enforce_repository_sandbox,
    };
    let probe = NativeProbe {
        root: root.filter(|_| request.enforce_repository_sandbox),
    };
    marknexia_navigation::resolve(Some(&request.href), &context, &probe).intent
}

/// True when `tab` shows the document of `epoch` now. With `active`, the
/// tab must also be the active one (a link acts on the active tab, as .NET
/// `GetActiveTabState`).
fn is_current(state: &Rc<RefCell<AppState>>, tab: u64, epoch: u64, active: bool) -> bool {
    if active && active_tab(state) != Some(tab) {
        return false;
    }
    let Some(session) = take_session(state) else {
        return false;
    };
    let current = session
        .host(tab)
        .and_then(|host| host.document_identity().ok())
        == Some((tab, epoch));
    restore_session(state, session);
    current
}

/// True while `tab` waits for the render of a document it navigates to:
/// its location already names that document, but the page still shows the
/// previous one. Clicks, hover and scroll reports from that page are then
/// ignored, so they never resolve against, or write into, the pending
/// location (history steps still apply).
fn awaiting_document(state: &Rc<RefCell<AppState>>, tab: u64) -> bool {
    state
        .try_borrow()
        .map_or(true, |state| state.chrome.navigating.contains_key(&tab))
}

/// `input::after_poll`: the link adapter's messages, in arrival order.
pub(in crate::window) fn page_messages(hwnd: HWND, messages: Vec<PageToHost>) {
    for message in messages {
        match message {
            PageToHost::OpenLink {
                tab_id,
                document_epoch,
                href,
                ..
            } => open_link(hwnd, tab_id, document_epoch, href),
            PageToHost::LinkHover {
                tab_id,
                document_epoch,
                href,
                ..
            } => hover(hwnd, tab_id, document_epoch, href.as_deref()),
            PageToHost::ScrollChanged {
                tab_id,
                document_epoch,
                top,
                ..
            } => scrolled(hwnd, tab_id, document_epoch, top),
            PageToHost::HistoryStep {
                tab_id,
                document_epoch,
                back,
                ..
            } => {
                if let Some(state) = state_of(hwnd)
                    && is_current(&state, tab_id, document_epoch, true)
                {
                    mouse_step(back);
                }
            }
            _ => {}
        }
    }
}

/// `chrome::after_poll`: navigations that bypassed the link adapter, as
/// (tab, epoch, document URI, requested URI). Each is followed like a
/// click on a link (LNK-7).
pub(in crate::window) fn navigation_requests(
    hwnd: HWND,
    requests: Vec<(u64, u64, String, String)>,
) {
    for (tab, epoch, document_uri, uri) in requests {
        let Some(origin) = document_origin(&document_uri) else {
            continue;
        };
        open_link(hwnd, tab, epoch, fallback_link_href(origin, &uri));
    }
}

/// .NET `HandleLinkClick`: resolve on a worker, act on the result.
fn open_link(hwnd: HWND, tab: u64, epoch: u64, href: String) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    // .NET returns on a blank destination before anything else.
    if href.trim().is_empty()
        || awaiting_document(&state, tab)
        || !is_current(&state, tab, epoch, true)
    {
        return;
    }
    timing_begin(tab, "link");
    let mut guard = state.borrow_mut();
    let chrome = &mut guard.chrome;
    let settings = settings_bridge::current(chrome.store.as_ref());
    chrome.link_generation = chrome.link_generation.wrapping_add(1);
    let generation = chrome.link_generation;
    let request = LinkRequest {
        href,
        // The start page has no file: .NET resolves against "".
        current_file: chrome
            .locations
            .get(&tab)
            .map(|location| location.path().to_path_buf())
            .unwrap_or_default(),
        active_root: chrome.repository_root.clone(),
        allow_external_links: settings.allow_external_links,
        enforce_repository_sandbox: settings.enforce_repository_sandbox,
    };
    let started = chrome.link.spawn("marknexia-link", move || {
        Some(WorkerResult::LinkResolved {
            tab,
            epoch,
            generation,
            intent: resolve_link(request),
        })
    });
    if !started {
        chrome.diagnostic(
            "Unable to open document: the link could not be resolved.",
            DiagnosticSeverity::Error,
        );
    }
}

/// The worker's resolution, back on the UI thread. Only the newest request
/// counts, and only while its document is still shown in the active tab.
pub(in crate::window) fn finish_link(
    hwnd: HWND,
    tab: u64,
    epoch: u64,
    generation: u64,
    intent: &NavigationOutcome,
) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    if state.borrow().chrome.link_generation != generation
        || awaiting_document(&state, tab)
        || !is_current(&state, tab, epoch, true)
    {
        return;
    }
    timing_step(tab, "resolved");
    match link_action(intent) {
        LinkAction::ScrollToAnchor(fragment) => {
            // .NET `OpenDocumentInTabAsync(active.FilePath, fragment)`: the
            // previous location is recorded when the location changes.
            // LNK-4: the page scrolls even when the anchor is unchanged.
            if let Some(location) = state.borrow_mut().chrome.locations.get_mut(&tab) {
                location.navigate_to(Some(&fragment));
            }
            scroll_location(&state, tab, Some(&fragment), 0);
        }
        LinkAction::OpenDocument { path, fragment } => {
            let previous = state.borrow().chrome.locations.get(&tab).cloned();
            let Some(previous) = previous else {
                // The start page: no tab history to replace; open normally.
                super::open_or_select(hwnd, path);
                return;
            };
            let mut next = previous.clone();
            next.navigate_document(path.clone(), fragment.as_deref());
            navigate(hwnd, &state, tab, previous, next, 0);
        }
        LinkAction::OpenExternal(uri) => {
            let started = state
                .borrow()
                .chrome
                .link
                .spawn("marknexia-external-link", move || {
                    Some(WorkerResult::ExternalOpened(
                        super::shell_ops::open_external(&uri),
                    ))
                });
            if !started {
                state
                    .borrow_mut()
                    .chrome
                    .diagnostic("The link could not be opened.", DiagnosticSeverity::Warning);
            }
        }
        LinkAction::Diagnostic(message) => {
            state
                .borrow_mut()
                .chrome
                .diagnostic(message.message, message.severity);
        }
        LinkAction::Nothing => {}
    }
    sync_ui(hwnd);
}

/// Shows `next` (a location for another document) in `tab`, which showed
/// `previous` (.NET `ReplaceActiveDocumentAsync`). The tab, its controller
/// and its history stay; the tab header follows the document; the render
/// runs on the pool and the controller navigates in place when it is ready,
/// so the old page stays until then. `top` is the scroll offset to restore
/// after the load (back and forward), else 0. A failed render restores
/// `previous` (`filter_render_outcome`).
pub(in crate::window) fn navigate(
    hwnd: HWND,
    state: &Rc<RefCell<AppState>>,
    tab: u64,
    previous: TabLocation,
    next: TabLocation,
    top: u32,
) {
    let path = next.path().to_path_buf();
    if let Err(error) = check_document_path(&path.to_string_lossy()) {
        let mut guard = state.borrow_mut();
        // Back and forward already moved the history: the tab keeps its
        // document, so it keeps its location too.
        guard.chrome.locations.insert(tab, previous);
        guard.chrome.diagnostic(
            format!("Unable to open document: {error}"),
            DiagnosticSeverity::Error,
        );
        drop(guard);
        sync_ui(hwnd);
        return;
    }
    {
        let mut guard = state.borrow_mut();
        guard.link_hover = None;
        let chrome = &mut guard.chrome;
        chrome.locations.insert(tab, next);
        // A second navigation before the first rendered keeps the oldest
        // location to restore.
        chrome.navigating.entry(tab).or_insert(previous);
        if top > 0 {
            chrome.pending_top.insert(tab, top);
        } else {
            chrome.pending_top.remove(&tab);
        }
    }
    rename_tab(state, tab, &path);
    timing_step(tab, "render");
    start_render(hwnd, state, tab, path);
    super::super::refresh_status(state);
    sync_ui(hwnd);
}

/// The tab header names the shown document.
pub(in crate::window) fn rename_tab(
    state: &Rc<RefCell<AppState>>,
    tab: u64,
    path: &std::path::Path,
) {
    let portable = Rc::clone(&state.borrow().portable);
    let id = portable
        .borrow()
        .tabs()
        .tabs()
        .iter()
        .find_map(|candidate| (candidate.id().get() == tab).then(|| candidate.id()));
    if let Some(id) = id {
        portable.borrow_mut().rename_tab(id, tab_title(path));
        super::super::refresh_tabs(state, None);
    }
}

/// Scrolls `tab`'s loaded document: to `top` when known (back and
/// forward), else to the anchor, else to the top. While the tab's document
/// is still loading (or rendering, or its controller is being created), the
/// scroll waits as a pending location (`pending_scroll`, `pending_top`) and
/// `chrome::after_poll` applies it after the load, so it is never lost.
pub(in crate::window) fn scroll_location(
    state: &Rc<RefCell<AppState>>,
    tab: u64,
    fragment: Option<&str>,
    top: u32,
) {
    let applied = take_session(state).is_some_and(|session| {
        let applied = session
            .host(tab)
            .filter(|host| host.document_loaded())
            .is_some_and(|host| {
                if top > 0 {
                    host.scroll_to_offset(top).is_ok()
                } else {
                    host.scroll_to_anchor(fragment).is_ok()
                }
            });
        restore_session(state, session);
        applied
    });
    if !applied {
        // `after_poll` scrolls to the location's fragment, or to `top`.
        let mut guard = state.borrow_mut();
        let chrome = &mut guard.chrome;
        chrome.pending_scroll.insert(tab);
        if top > 0 {
            chrome.pending_top.insert(tab, top);
        } else {
            chrome.pending_top.remove(&tab);
        }
    }
}

/// Back or forward (.NET `Back_Click`, `Forward_Click`): the entry's
/// document is shown in the same tab, then its scroll offset (LNK-3), else
/// its anchor. An entry in the same document only scrolls (CHROME-4).
pub(in crate::window) fn history_step(hwnd: HWND, back: bool) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let Some(tab) = active_tab(&state) else {
        return;
    };
    let step = {
        let mut guard = state.borrow_mut();
        guard.chrome.locations.get_mut(&tab).and_then(|location| {
            let previous = location.clone();
            let entry = if back {
                location.go_back()
            } else {
                location.go_forward()
            }?;
            Some((previous, location.clone(), entry))
        })
    };
    if let Some((previous, next, entry)) = step {
        let top = crate::chrome_model::history_offset(entry.scroll_top);
        if same_document_path(previous.path(), next.path()) {
            scroll_location(&state, tab, entry.fragment.as_deref(), top);
        } else {
            // `navigate` stores `next` again; the history already moved.
            timing_begin(tab, if back { "back" } else { "forward" });
            navigate(hwnd, &state, tab, previous, next, top);
            return;
        }
    }
    sync_ui(hwnd);
}

/// The mouse back and forward buttons, from the page or the shell.
fn mouse_step(back: bool) {
    let now = Instant::now();
    let repeated = LAST_MOUSE_STEP.with(|last| {
        let repeated = last
            .get()
            .is_some_and(|previous| now.duration_since(previous) < MOUSE_STEP_DEDUPE);
        last.set(Some(now));
        repeated
    });
    if !repeated {
        queue_action(ChromeAction::Command(if back {
            ChromeCommand::Back
        } else {
            ChromeCommand::Forward
        }));
    }
}

/// `chrome::handle_message`: the mouse back and forward buttons over the
/// shell's own windows (`WM_XBUTTONUP`, and `WM_APPCOMMAND` from child
/// controls and keyboards with browser keys).
pub(in crate::window) fn handle_message(
    _hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    match message {
        WM_XBUTTONUP => {
            mouse_step((wparam.0 >> 16) & 0xFFFF == XBUTTON1);
            Some(LRESULT(1))
        }
        WM_APPCOMMAND => {
            // GET_APPCOMMAND_LPARAM: the high word without the device bits.
            let command = ((lparam.0 as usize >> 16) & 0x0FFF) as u32;
            match command {
                APPCOMMAND_BROWSER_BACKWARD => {
                    mouse_step(true);
                    Some(LRESULT(1))
                }
                APPCOMMAND_BROWSER_FORWARD => {
                    mouse_step(false);
                    Some(LRESULT(1))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The hovered or focused link's target in the status bar.
fn hover(hwnd: HWND, tab: u64, epoch: u64, href: Option<&str>) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    if awaiting_document(&state, tab) || !is_current(&state, tab, epoch, true) {
        return;
    }
    let text = href.map(|href| {
        let guard = state.borrow();
        let chrome = &guard.chrome;
        let settings = settings_bridge::current(chrome.store.as_ref());
        let current = chrome
            .locations
            .get(&tab)
            .map(|location| location.path().to_path_buf())
            .unwrap_or_default();
        let root = chrome
            .repository_root
            .clone()
            .filter(|root| reveal_allowed(&current, Some(root)));
        link_status_text(
            href,
            &current,
            root.as_deref(),
            settings.allow_external_links,
            settings.enforce_repository_sandbox,
        )
    });
    let changed = {
        let mut guard = state.borrow_mut();
        let changed = guard.link_hover != text;
        guard.link_hover = text;
        changed
    };
    if changed {
        super::super::refresh_status(&state);
    }
}

/// The page's scroll offset, kept for the next history entry.
fn scrolled(hwnd: HWND, tab: u64, epoch: u64, top: u32) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    if !awaiting_document(&state, tab)
        && is_current(&state, tab, epoch, false)
        && let Some(location) = state.borrow_mut().chrome.locations.get_mut(&tab)
    {
        location.set_scroll_top(top);
    }
}

/// `WorkerResult::ExternalOpened` failed.
pub(in crate::window) fn external_failed(chrome: &mut super::ChromeState, error: &str) {
    chrome.diagnostic(
        format!("The link could not be opened: {error}"),
        DiagnosticSeverity::Warning,
    );
}
