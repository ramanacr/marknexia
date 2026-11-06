//! Input wiring for Phase 2 tasks 3 to 5: find in page, drag and drop,
//! single-instance activation, and tab close and reorder.
//!
//! This is a child module of `window`, so it uses the shell's private state
//! and helpers directly; the hooks in `window.rs` are one-line calls into
//! this module. The portable rules live in `crate::find`, `crate::dragdrop`,
//! `crate::single_instance` and `crate::accessibility`.
//!
//! The invariants of `window.rs` hold here too: no `AppState` borrow spans a
//! call that can re-enter the window procedure (WebView2 COM, `SetFocus`,
//! `SetWindowTextW` on the find box, capture changes), and WebView2 is
//! called only from top-level dispatch, never inside a UIA call.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

use marknexia_core::text::eq_ignore_case;
use marknexia_webview::protocol::{HostToPage, PageToHost};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, VK_MENU},
    Shell::{DragAcceptFiles, HDROP},
    WindowsAndMessaging::{GetSystemMetrics, SM_CXDRAG, SendMessageW, WM_APP},
};

use super::*;
use crate::{
    accessibility::{TabHit, close_button_slot, hit_test_tab, native::CloseTab, tab_index_at},
    dragdrop::{self, DropAction, DropQueue},
    find::{
        DOCUMENT_LOADING, FIND_UNAVAILABLE, FindCommand, FindModel, FindStart,
        native::{self as find_native, ButtonColors, FindControls, Glyph},
        route_find_key,
    },
    single_instance::{
        self, KEY_VARIABLE,
        native::{ActivationQueue, ActivationServer, Claim, PrimaryInstance, ShellStartupProbe},
    },
};

/// Posted by the activation server thread: activations are queued.
const WM_ACTIVATION_REQUEST: u32 = WM_APP + 0x100;
/// Posted by UIA `Invoke` on a close button: close the tab in `wparam`.
const WM_CLOSE_TAB_REQUEST: u32 = WM_APP + 0x101;
/// Posted by a drop classification thread: drop actions are queued.
const WM_DROPS_CLASSIFIED: u32 = WM_APP + 0x102;
/// The find box's control ID (`initialize` creates it).
const FIND_EDIT_ID: usize = 1004;
const WM_DROPFILES: u32 = 0x0233;
const EM_SETSEL: u32 = 0x00B1;
const PROTOCOL: u16 = 1;

/// A press on the tab strip, until the button is released.
#[derive(Clone, Copy, Debug)]
pub(super) struct TabPress {
    tab: TabId,
    /// The press started on the tab's close button.
    close: bool,
    x: i32,
    dragging: bool,
}

/// State owned by this module inside `AppState`.
#[derive(Default)]
pub(super) struct InputState {
    find: FindModel,
    find_controls: Option<FindControls>,
    /// True while the shell itself sets the find box text, so `EN_CHANGE`
    /// does not count as user input.
    find_text_programmatic: bool,
    /// Each document tab's path, for .NET's "already open" check.
    tab_paths: BTreeMap<u64, PathBuf>,
    /// The folder dropped last (.NET `_activeRepositoryRoot`). The sidebar
    /// shows it once the repository tree is wired.
    repository_root: Option<PathBuf>,
    drops: DropQueue,
    /// Validated page messages, queued by the page observer during
    /// `WebViewSession::poll` and drained after the session is restored.
    page_messages: Rc<RefCell<VecDeque<PageToHost>>>,
    activations: ActivationQueue,
    activation_server: Option<ActivationServer>,
    /// The strong owner of the UIA close callback (the provider holds a
    /// weak reference, like `uia_selection`).
    close_request: Option<Rc<CloseTab>>,
    tab_press: Option<TabPress>,
}

impl InputState {
    fn forget_tab(&mut self, tab_id: u64) {
        self.find.forget(tab_id);
        self.tab_paths.remove(&tab_id);
    }
}

fn handle(hwnd: HWND) -> Option<Rc<RefCell<AppState>>> {
    // SAFETY: `hwnd` is this thread's shell window; the helper returns a
    // scoped strong reference and no borrow.
    unsafe { app_state_handle(hwnd) }
}

fn active_tab(state: &Rc<RefCell<AppState>>) -> Option<TabId> {
    let portable = Rc::clone(&state.borrow().portable);
    portable.borrow().active_tab()
}

fn tab_by_raw(state: &Rc<RefCell<AppState>>, raw: u64) -> Option<TabId> {
    let portable = Rc::clone(&state.borrow().portable);
    let app = portable.borrow();
    app.tabs()
        .tabs()
        .iter()
        .find(|tab| tab.id().get() == raw)
        .map(|tab| tab.id())
}

fn show_notice(state: &Rc<RefCell<AppState>>, text: &str) {
    state.borrow_mut().notice = Some(text.to_owned());
    refresh_status(state);
}

// ---------------------------------------------------------------------------
// Startup and shutdown hooks
// ---------------------------------------------------------------------------

/// Decides single instance before any window exists (.NET `OnLaunched`).
pub(super) fn claim_instance() -> Claim {
    let key = single_instance::instance_key(std::env::var_os(KEY_VARIABLE).as_deref());
    single_instance::native::claim(&key, &single_instance::native::current_activation())
}

/// Window-creation hook: the find bar, file drops, and the UIA close
/// callback (returned for `NativeAccessibility::new`).
pub(super) fn initialize(
    hwnd: HWND,
    instance: HINSTANCE,
    state: &Rc<RefCell<AppState>>,
    find_edit: HWND,
) -> Result<Rc<CloseTab>, WindowError> {
    let controls = FindControls::create(hwnd, instance, find_edit).map_err(WindowError::Child)?;
    // SAFETY: this thread's own top-level window.
    unsafe { DragAcceptFiles(hwnd, true) };
    let window = hwnd.0 as isize;
    let close: Rc<CloseTab> = Rc::new(move |tab: TabId| {
        // SAFETY: posts to this thread's own window; no pointers.
        unsafe {
            PostMessageW(
                Some(HWND(window as *mut c_void)),
                WM_CLOSE_TAB_REQUEST,
                WPARAM(tab.get() as usize),
                LPARAM(0),
            )
        }
        .is_ok()
    });
    let mut state = state.borrow_mut();
    state.input.find_controls = Some(controls);
    state.input.close_request = Some(Rc::clone(&close));
    Ok(close)
}

/// The application page observer: it only queues. It runs inside
/// `WebViewSession::poll`, while the session is checked out.
pub(super) fn page_observer(state: &Rc<RefCell<AppState>>) -> Rc<dyn Fn(PageToHost)> {
    const MAX_QUEUED: usize = 256;
    let queue = Rc::clone(&state.borrow().input.page_messages);
    Rc::new(move |message| {
        if let Ok(mut queue) = queue.try_borrow_mut()
            && queue.len() < MAX_QUEUED
        {
            queue.push_back(message);
        }
    })
}

/// Records the startup documents' paths (for the "already open" check).
pub(super) fn remember_startup_paths(
    state: &Rc<RefCell<AppState>>,
    documents: &[(TabId, PathBuf)],
) {
    let mut state = state.borrow_mut();
    for (tab_id, path) in documents {
        state.input.tab_paths.insert(tab_id.get(), path.clone());
    }
}

/// Starts the activation server once the window exists.
pub(super) fn serve_activations(hwnd: HWND, primary: Option<PrimaryInstance>) {
    let (Some(primary), Some(state)) = (primary, handle(hwnd)) else {
        return;
    };
    let queue = state.borrow().input.activations.clone();
    let server = primary.serve(hwnd, WM_ACTIVATION_REQUEST, queue);
    state.borrow_mut().input.activation_server = server;
}

/// `close_state` hook: stops the activation server first, so a new launch
/// becomes the first instance instead of handing off to this one.
pub(super) fn shutdown(state: &Rc<RefCell<AppState>>) {
    let server = state.borrow_mut().input.activation_server.take();
    drop(server);
}

/// The folder most recently dropped onto the shell.
#[allow(dead_code)] // Read by the repository sidebar once it is wired.
pub(super) fn repository_root(state: &Rc<RefCell<AppState>>) -> Option<PathBuf> {
    state.borrow().input.repository_root.clone()
}

// ---------------------------------------------------------------------------
// Window messages
// ---------------------------------------------------------------------------

/// Messages this module owns.
pub(super) fn handles(message: u32) -> bool {
    matches!(
        message,
        WM_ACTIVATION_REQUEST | WM_CLOSE_TAB_REQUEST | WM_DROPS_CLASSIFIED | WM_DROPFILES
    )
}

pub(super) fn on_message(hwnd: HWND, message: u32, wparam: WPARAM) -> LRESULT {
    match message {
        WM_ACTIVATION_REQUEST => on_activation(hwnd),
        WM_CLOSE_TAB_REQUEST => {
            if let Some(tab) = handle(hwnd).and_then(|state| tab_by_raw(&state, wparam.0 as u64)) {
                let _ = close_tab(hwnd, tab);
            }
        }
        WM_DROPS_CLASSIFIED => on_drops_classified(hwnd),
        WM_DROPFILES => on_drop_files(hwnd, HDROP(wparam.0 as *mut c_void)),
        _ => {}
    }
    LRESULT(0)
}

/// `WM_COMMAND` from the find box (`EN_CHANGE`) and the find buttons
/// (`BN_CLICKED`). Returns true when handled.
pub(super) fn on_command(hwnd: HWND, wparam: WPARAM) -> bool {
    let id = wparam.0 & 0xffff;
    let code = ((wparam.0 >> 16) & 0xffff) as u32;
    let command = match (id as isize, code) {
        (find_native::PREVIOUS_ID, 0) => FindCommand::Previous,
        (find_native::NEXT_ID, 0) => FindCommand::Next,
        (find_native::CLOSE_ID, 0) => FindCommand::Close,
        _ if id == FIND_EDIT_ID && code == find_native::EN_CHANGE => {
            on_find_text_changed(hwnd);
            return true;
        }
        _ => return false,
    };
    run_find_command(hwnd, command);
    true
}

// ---------------------------------------------------------------------------
// Find in page (.NET MainWindow.Search.cs)
// ---------------------------------------------------------------------------

fn find_context(hwnd: HWND) -> Option<(Rc<RefCell<AppState>>, FindControls)> {
    let state = handle(hwnd)?;
    let controls = state.borrow().input.find_controls?;
    Some((state, controls))
}

fn find_bar_visible(state: &Rc<RefCell<AppState>>) -> bool {
    let portable = Rc::clone(&state.borrow().portable);
    portable.borrow().find_bar_visible()
}

/// Message-loop key hook, before the shell's own key routing.
pub(super) fn route_key(hwnd: HWND, target: HWND, key: u16) -> bool {
    // SAFETY: reads this thread's keyboard state.
    let down = |key: u16| unsafe { GetKeyState(i32::from(key)) } < 0;
    let Some((_, controls)) = find_context(hwnd) else {
        return false;
    };
    let command = route_find_key(
        key,
        down(VK_CONTROL.0),
        down(VK_SHIFT.0),
        down(VK_MENU.0),
        target == controls.edit,
    );
    command.is_some_and(|command| {
        run_find_command(hwnd, command);
        true
    })
}

/// A chord the WebView handed back (Ctrl+F, F3, Shift+F3).
pub(super) fn route_accelerator(hwnd: HWND, key: u16, ctrl: bool, shift: bool) -> bool {
    match route_find_key(key, ctrl, shift, false, false) {
        Some(command) => {
            run_find_command(hwnd, command);
            true
        }
        None => false,
    }
}

fn run_find_command(hwnd: HWND, command: FindCommand) {
    let Some((state, _)) = find_context(hwnd) else {
        return;
    };
    let visible = find_bar_visible(&state);
    match command {
        FindCommand::Toggle if visible => close_search(hwnd),
        FindCommand::Toggle => open_search(hwnd),
        FindCommand::Close => close_search(hwnd),
        FindCommand::Next | FindCommand::Previous => {
            if !visible {
                open_search(hwnd);
            }
            find_in_document(hwnd, command == FindCommand::Previous);
        }
    }
}

fn set_find_bar_visible(hwnd: HWND, state: &Rc<RefCell<AppState>>, visible: bool) {
    let (portable, dpi) = {
        let state = state.borrow();
        (Rc::clone(&state.portable), state.dpi)
    };
    portable.borrow_mut().set_find_bar_visible(visible);
    reflow(hwnd, dpi);
}

/// .NET `ToggleSearch_Click` (opening).
fn open_search(hwnd: HWND) {
    let Some((state, controls)) = find_context(hwnd) else {
        return;
    };
    set_find_bar_visible(hwnd, &state, true);
    refresh_find(&state);
    // SAFETY: live child of this thread's window; no borrow is held.
    unsafe {
        let _ = SetFocus(Some(controls.edit));
        let _ = SendMessageW(controls.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
    }
}

/// .NET `CloseSearch_Click`: hide, clear, store an empty result for the
/// active tab, and clear the page selection.
fn close_search(hwnd: HWND) {
    let Some((state, controls)) = find_context(hwnd) else {
        return;
    };
    // SAFETY: reads this thread's focus.
    let focused = unsafe { GetFocus() };
    let had_focus = [
        controls.edit,
        controls.previous,
        controls.next,
        controls.close,
    ]
    .contains(&focused);
    set_find_bar_visible(hwnd, &state, false);
    set_find_text(&state, controls.edit, "");
    controls.set_count("");
    if let Some(tab) = active_tab(&state).map(TabId::get) {
        state.borrow_mut().input.find.close(tab);
        let _ = post_to_page(&state, tab, |document_epoch| HostToPage::ClearFind {
            protocol: PROTOCOL,
            tab_id: tab,
            document_epoch,
        });
    }
    if had_focus && let Some(c) = state.borrow().controls {
        // A hidden control keeps no focus; the tab strip takes it.
        let _ = unsafe { SetFocus(Some(c.tabs)) };
    }
}

/// .NET `FindInRenderedDocumentAsync`, up to the bridge call; the result
/// arrives as a page message (`apply_page_messages`).
fn find_in_document(hwnd: HWND, backwards: bool) {
    let Some((state, controls)) = find_context(hwnd) else {
        return;
    };
    let Some(tab) = active_tab(&state).map(TabId::get) else {
        return;
    };
    let query = controls.query();
    let start = state.borrow_mut().input.find.begin(tab, &query);
    match start {
        FindStart::Loading => controls.set_count(DOCUMENT_LOADING),
        FindStart::Post { request, query } => {
            let posted = post_to_page(&state, tab, |document_epoch| HostToPage::Find {
                protocol: PROTOCOL,
                tab_id: tab,
                document_epoch,
                request_id: request.0,
                query,
                backwards,
            });
            if posted.is_err() {
                controls.set_count("");
                show_notice(&state, FIND_UNAVAILABLE);
            }
        }
    }
}

/// Posts a typed message to `tab`'s current document.
fn post_to_page(
    state: &Rc<RefCell<AppState>>,
    tab: u64,
    message: impl FnOnce(u64) -> HostToPage,
) -> Result<(), ()> {
    let session = take_session(state).ok_or(())?;
    let result = session.host(tab).ok_or(()).and_then(|host| {
        let (_, epoch) = host.document_identity().map_err(drop)?;
        host.post_message(&message(epoch)).map_err(drop)
    });
    restore_session(state, session);
    result
}

/// Sets the find box text without counting it as user input.
fn set_find_text(state: &Rc<RefCell<AppState>>, edit: HWND, text: &str) {
    if find_native::read_text(edit) == text {
        return;
    }
    state.borrow_mut().input.find_text_programmatic = true;
    find_native::set_text(edit, text);
    state.borrow_mut().input.find_text_programmatic = false;
}

/// .NET `SearchInputBox_TextChanged`.
fn on_find_text_changed(hwnd: HWND) {
    let Some((state, controls)) = find_context(hwnd) else {
        return;
    };
    if state.borrow().input.find_text_programmatic {
        return;
    }
    if let Some(tab) = active_tab(&state).map(TabId::get) {
        let query = controls.query();
        state.borrow_mut().input.find.set_query(tab, &query);
    }
    controls.set_count("");
}

/// .NET `RefreshSearchForActiveTab`, on tab selection changes.
pub(super) fn refresh_find(state: &Rc<RefCell<AppState>>) {
    let Some(controls) = state.borrow().input.find_controls else {
        return;
    };
    let active = active_tab(state).map(TabId::get);
    let (query, text) = {
        let state = state.borrow();
        (
            active
                .map(|tab| state.input.find.query(tab).to_owned())
                .unwrap_or_default(),
            state.input.find.refresh_text(active),
        )
    };
    set_find_text(state, controls.edit, &query);
    controls.set_count(&text);
}

/// A new document joined `tab` (.NET `NavigationStarting`).
pub(super) fn document_replaced(state: &Rc<RefCell<AppState>>, tab: u64) {
    state.borrow_mut().input.find.invalidate(tab);
}

/// `poll_webview` hook, after the session is restored: applies queued page
/// messages and marks loaded documents ready (.NET `NavigationCompleted`,
/// which re-runs the active tab's search when the bar is visible).
pub(super) fn after_poll(hwnd: HWND) {
    let Some((state, controls)) = find_context(hwnd) else {
        return;
    };
    let messages: Vec<PageToHost> = {
        let queue = Rc::clone(&state.borrow().input.page_messages);
        queue.borrow_mut().drain(..).collect()
    };
    let active = active_tab(&state).map(TabId::get);
    let visible = find_bar_visible(&state);
    for message in messages {
        let text = match message {
            PageToHost::FindResult {
                tab_id,
                request_id,
                query,
                match_count,
                current_match,
                ..
            } => state.borrow_mut().input.find.apply_result(
                active,
                visible,
                tab_id,
                marknexia_core::search_state::SearchRequestId(request_id),
                &query,
                match_count as usize,
                current_match as usize,
            ),
            PageToHost::FindUnavailable {
                tab_id, request_id, ..
            } => state.borrow().input.find.apply_unavailable(
                active,
                tab_id,
                marknexia_core::search_state::SearchRequestId(request_id),
            ),
            // Links, copy and focus are not wired yet.
            _ => None,
        };
        if let Some(text) = text {
            controls.set_count(&text);
        }
    }
    // Newly loaded documents. The session is taken only to read state.
    let tabs: Vec<u64> = {
        let portable = Rc::clone(&state.borrow().portable);
        let app = portable.borrow();
        app.tabs().tabs().iter().map(|tab| tab.id().get()).collect()
    };
    let pending: Vec<u64> = {
        let state = state.borrow();
        tabs.into_iter()
            .filter(|tab| !state.input.find.is_ready(*tab))
            .collect()
    };
    if pending.is_empty() {
        return;
    }
    let Some(session) = take_session(&state) else {
        return;
    };
    let loaded: Vec<u64> = pending
        .into_iter()
        .filter(|tab| {
            session
                .host(*tab)
                .is_some_and(|host| host.document_loaded())
        })
        .collect();
    restore_session(&state, session);
    let mut rerun = false;
    for tab in loaded {
        rerun |= state
            .borrow_mut()
            .input
            .find
            .document_loaded(tab, active == Some(tab), visible);
    }
    if rerun {
        find_in_document(hwnd, false);
    }
}

/// `WM_DRAWITEM` hook for the find buttons.
pub(super) fn draw_find_button(hwnd: HWND, item: &DRAWITEMSTRUCT) -> bool {
    let Some(state) = handle(hwnd) else {
        return false;
    };
    let Ok(borrowed) = state.try_borrow() else {
        return false;
    };
    let Some(controls) = borrowed.input.find_controls else {
        return false;
    };
    if !controls.is_button(item.hwndItem) {
        return false;
    }
    let colors = palette(borrowed.theme);
    let dpi = borrowed.dpi;
    drop(borrowed);
    let colors = ButtonColors {
        face: native_color(colors.elevated),
        pressed: native_color(colors.accent_pressed),
        text: native_color(colors.text),
        on_pressed: native_color(colors.on_accent),
        border: native_color(colors.border),
    };
    find_native::draw_button(&controls, item, &colors, dpi)
}

/// `reflow` hook: lays out the find bar inside the shell layout's rect.
pub(super) fn layout_find_bar(state: &Rc<RefCell<AppState>>, bar: PixelRect, dpi: u32) {
    let controls = state.borrow().input.find_controls;
    if let Some(controls) = controls {
        controls.layout(bar, dpi);
    }
}

// ---------------------------------------------------------------------------
// Tabs: close, reorder, pointer input, painting
// ---------------------------------------------------------------------------

/// Closes `tab`, active or not (.NET `CloseTab`): its WebView document is
/// removed first, so a failure leaves the tab open. Raises the UIA
/// structure event after the switch to the next tab.
pub(super) fn close_tab(hwnd: HWND, tab: TabId) -> bool {
    let Some(state) = handle(hwnd) else {
        return false;
    };
    // An outer call holds the session: the document cannot be removed now.
    if state.borrow().webview_checked_out || tab_by_raw(&state, tab.get()).is_none() {
        return false;
    }
    let raw = tab.get();
    if let Some(mut session) = take_session(&state) {
        // A tab whose render is still pending has no document yet.
        let result = if session.has_document(raw) {
            session.remove_document(raw)
        } else {
            Ok(())
        };
        restore_session(&state, session);
        if result.is_err() {
            return false;
        }
    }
    {
        let mut state = state.borrow_mut();
        // Any queued or in-flight render for the closed tab is now stale.
        state.renders.cancel(raw);
        state.rendered_tabs.remove(&raw);
        state.tab_errors.remove(&raw);
        if state.webview_active == Some(raw) {
            state.webview_active = None;
        }
        // Closed before it rendered: no longer a startup candidate.
        let signal = state.startup.failed(raw);
        state.apply_startup(signal);
        state.input.forget_tab(raw);
    }
    let portable = Rc::clone(&state.borrow().portable);
    let previous = portable.borrow().active_tab();
    let closed = portable.borrow_mut().close_tab(tab);
    if !closed {
        return false;
    }
    let active = portable.borrow().active_tab();
    if previous == Some(tab)
        && let Some(active) = active
    {
        // A failure is reported by `select_webview` in the status bar.
        let _ = select_webview(&state, active.get());
    }
    let a11y = state.borrow().accessibility.clone();
    if let Some(a11y) = a11y {
        a11y.notify_children_changed();
    }
    refresh_tabs(&state, active.filter(|id| Some(*id) != previous));
    if previous == Some(tab) {
        refresh_find(&state);
    }
    true
}

/// Moves `tab` to `destination`; raises `ChildrenReordered` when the order
/// changed.
fn reorder_tab(state: &Rc<RefCell<AppState>>, tab: TabId, destination: usize) -> bool {
    let portable = Rc::clone(&state.borrow().portable);
    let moved = portable.borrow_mut().reorder_tab(tab, destination);
    if moved {
        let a11y = state.borrow().accessibility.clone();
        if let Some(a11y) = a11y {
            a11y.notify_children_reordered();
        }
        refresh_tabs(state, None);
    }
    moved
}

/// The strip's client size and its parent's state.
fn strip_context(strip: HWND) -> Option<(HWND, Rc<RefCell<AppState>>, i32, i32)> {
    // SAFETY: `strip` is this thread's tab strip.
    let parent = unsafe { windows::Win32::UI::WindowsAndMessaging::GetParent(strip) }.ok()?;
    let state = handle(parent)?;
    let mut client = RECT::default();
    unsafe { GetClientRect(strip, &mut client) }.ok()?;
    Some((
        parent,
        state,
        client.right - client.left,
        client.bottom - client.top,
    ))
}

fn tab_ids(state: &Rc<RefCell<AppState>>) -> Vec<TabId> {
    let portable = Rc::clone(&state.borrow().portable);
    let app = portable.borrow();
    app.tabs().tabs().iter().map(|tab| tab.id()).collect()
}

/// `WM_LBUTTONDOWN` on the tab strip: a body press selects (as before) and
/// may start a drag; a close-button press closes on release.
pub(super) fn on_tab_button_down(strip: HWND, x: i32) {
    let Some((parent, state, width, height)) = strip_context(strip) else {
        return;
    };
    let tabs = tab_ids(&state);
    let Some(hit) = hit_test_tab(width, height, tabs.len(), x) else {
        return;
    };
    let press = match hit {
        TabHit::Close(index) => {
            let portable = Rc::clone(&state.borrow().portable);
            portable.borrow_mut().note_pointer_input();
            TabPress {
                tab: tabs[index],
                close: true,
                x,
                dragging: false,
            }
        }
        TabHit::Body(index) => {
            select_tab_at(parent, x);
            // SAFETY: this thread's tab strip; no borrow is held.
            let _ = unsafe { SetFocus(Some(strip)) };
            TabPress {
                tab: tabs[index],
                close: false,
                x,
                dragging: false,
            }
        }
    };
    state.borrow_mut().input.tab_press = Some(press);
    // SAFETY: this thread's window; capture ends in WM_LBUTTONUP.
    let _ = unsafe { SetCapture(strip) };
}

/// `WM_MOUSEMOVE`: past the system drag distance, the pressed tab follows
/// the pointer (.NET `CanReorderTabs`).
pub(super) fn on_tab_mouse_move(strip: HWND, x: i32) {
    let Some((_, state, width, _)) = strip_context(strip) else {
        return;
    };
    let Some(mut press) = state.borrow().input.tab_press else {
        return;
    };
    if press.close {
        return;
    }
    if !press.dragging {
        // SAFETY: plain metric query.
        let threshold = unsafe { GetSystemMetrics(SM_CXDRAG) }.max(1);
        if (x - press.x).abs() < threshold {
            return;
        }
        press.dragging = true;
        state.borrow_mut().input.tab_press = Some(press);
    }
    let tabs = tab_ids(&state);
    let (Some(destination), Some(current)) = (
        tab_index_at(width, tabs.len(), x),
        tabs.iter().position(|tab| *tab == press.tab),
    ) else {
        return;
    };
    if destination != current {
        let _ = reorder_tab(&state, press.tab, destination);
    }
}

/// `WM_LBUTTONUP`: ends a drag, or closes the tab when the press started
/// and ended on the same close button.
pub(super) fn on_tab_button_up(strip: HWND, x: i32) {
    let Some((parent, state, width, height)) = strip_context(strip) else {
        return;
    };
    let press = state.borrow_mut().input.tab_press.take();
    // SAFETY: releases this thread's capture; WM_CAPTURECHANGED runs here
    // and finds no press.
    let _ = unsafe { ReleaseCapture() };
    let Some(press) = press.filter(|press| press.close) else {
        return;
    };
    let tabs = tab_ids(&state);
    if let Some(TabHit::Close(index)) = hit_test_tab(width, height, tabs.len(), x)
        && tabs[index] == press.tab
    {
        let _ = close_tab(parent, press.tab);
    }
}

/// `WM_MBUTTONUP`: a middle click anywhere on a tab closes it, like a
/// closable WinUI `TabViewItem`.
pub(super) fn on_tab_middle_click(strip: HWND, x: i32) {
    let Some((parent, state, width, height)) = strip_context(strip) else {
        return;
    };
    let tabs = tab_ids(&state);
    if let Some(TabHit::Body(index) | TabHit::Close(index)) =
        hit_test_tab(width, height, tabs.len(), x)
    {
        let portable = Rc::clone(&state.borrow().portable);
        portable.borrow_mut().note_pointer_input();
        let _ = close_tab(parent, tabs[index]);
    }
}

/// `WM_CAPTURECHANGED`: a lost capture cancels the press.
pub(super) fn on_tab_capture_lost(strip: HWND) {
    if let Some((_, state, _, _)) = strip_context(strip) {
        state.borrow_mut().input.tab_press = None;
    }
}

/// Paints a tab's title, clipped before its close button, and the close
/// glyph. Text starts where it did before close buttons existed.
pub(super) fn paint_tab_content(dc: HDC, rect: &RECT, title: &str, text_color: COLORREF, dpi: u32) {
    use windows::Win32::Graphics::Gdi::{DT_END_ELLIPSIS, DT_NOPREFIX};
    let scale = |dip: u32| (dip.saturating_mul(dpi) / 96) as i32;
    let close = close_button_slot(rect.left, rect.right, rect.bottom - rect.top);
    let mut text_rect = RECT {
        left: rect.left.saturating_add(scale(12)),
        top: rect.top.saturating_add(scale(12)),
        right: close
            .map_or(rect.right, |(left, _)| left)
            .saturating_sub(scale(4)),
        bottom: rect.bottom,
    };
    let mut text: Vec<u16> = title.encode_utf16().collect();
    // SAFETY: `dc` is the strip's paint DC for this WM_PAINT.
    unsafe {
        SetTextColor(dc, text_color);
        SetBkMode(dc, TRANSPARENT);
        if text_rect.right > text_rect.left {
            DrawTextW(
                dc,
                &mut text,
                &mut text_rect,
                DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
    }
    if let Some((left, right)) = close {
        let glyph = RECT {
            left,
            top: rect.top,
            right,
            bottom: rect.bottom,
        };
        find_native::draw_glyph(
            dc,
            &glyph,
            Glyph::Cross,
            text_color,
            scale(10),
            scale(1).max(1),
        );
    }
}

// ---------------------------------------------------------------------------
// Documents: the "already open" check
// ---------------------------------------------------------------------------

/// .NET `OpenDocumentInTabCoreAsync`: a path that is already open (ordinal
/// case-insensitive) selects its tab instead of opening another. Returns
/// true when it did.
pub(super) fn select_open_document(hwnd: HWND, state: &Rc<RefCell<AppState>>, path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    let existing = {
        let state = state.borrow();
        state
            .input
            .tab_paths
            .iter()
            .find(|(_, open)| open.to_str().is_some_and(|open| eq_ignore_case(open, text)))
            .map(|(tab, _)| *tab)
    };
    let Some(existing) = existing.and_then(|raw| tab_by_raw(state, raw)) else {
        return false;
    };
    let _ = select_tab_id(hwnd, existing.get(), false);
    true
}

pub(super) fn remember_path(state: &Rc<RefCell<AppState>>, tab: TabId, path: &Path) {
    state
        .borrow_mut()
        .input
        .tab_paths
        .insert(tab.get(), path.to_path_buf());
}

// ---------------------------------------------------------------------------
// Drag and drop (.NET Root_DragOver / Root_Drop)
// ---------------------------------------------------------------------------

fn on_drop_files(hwnd: HWND, drop: HDROP) {
    // SAFETY: `drop` is this WM_DROPFILES message's handle; it is released
    // inside and not used again.
    let Some(path) = (unsafe { dragdrop::native::take_first_path(drop) }) else {
        return;
    };
    let Some(state) = handle(hwnd) else {
        return;
    };
    let path = match dragdrop::gate(&path) {
        Ok(path) => path,
        Err(message) => {
            show_notice(&state, message);
            return;
        }
    };
    let queue = state.borrow().input.drops.clone();
    if !queue.reserve() {
        return;
    }
    let window = hwnd.0 as isize;
    let worker_queue = queue.clone();
    let spawned = std::thread::Builder::new()
        .name("marknexia-drop".to_owned())
        .spawn(move || {
            let action = std::panic::catch_unwind(|| dragdrop::classify(&path)).ok();
            worker_queue.complete(action);
            // SAFETY: posts to the shell window; after it is destroyed the
            // post just fails.
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(window as *mut c_void)),
                    WM_DROPS_CLASSIFIED,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
        });
    if spawned.is_err() {
        queue.complete(None);
    }
}

fn on_drops_classified(hwnd: HWND) {
    let Some(state) = handle(hwnd) else {
        return;
    };
    let actions = state.borrow().input.drops.drain();
    for action in actions {
        match action {
            DropAction::OpenDocument(path) => open_document(hwnd, path),
            DropAction::NotMarkdown => show_notice(&state, dragdrop::NOT_MARKDOWN),
            DropAction::OpenRepository { root, readme } => {
                let root = std::path::absolute(&root).unwrap_or(root);
                state.borrow_mut().input.repository_root = Some(root);
                if let Some(readme) = readme {
                    open_document(hwnd, readme);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Single instance (.NET App.MainInstance_Activated)
// ---------------------------------------------------------------------------

fn on_activation(hwnd: HWND) {
    let Some(state) = handle(hwnd) else {
        return;
    };
    let activations: Vec<_> = {
        let queue = state.borrow().input.activations.clone();
        let mut queue = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.drain(..).collect()
    };
    let own: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    for activation in activations {
        single_instance::native::bring_to_front(hwnd);
        let resolved = single_instance::resolve_activation(&activation, &own, &ShellStartupProbe);
        let Some(path) = resolved else {
            continue;
        };
        match crate::documents::check_document_path(&path) {
            Ok(()) => open_document(hwnd, PathBuf::from(path)),
            Err(_) => show_notice(&state, dragdrop::REFUSED),
        }
    }
}
