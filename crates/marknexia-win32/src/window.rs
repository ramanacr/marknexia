//! Raw Win32 shell. All HWND and WebView2 COM state stays on the owning STA.

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, mpsc},
};

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::PageIdentity;
use marknexia_webview::{
    environment::StaApartment,
    host::{HostColor, ViewportBounds},
    policy::HostDocument,
    protocol::PageToHost,
    session::WebViewSession,
};
use windows::{
    Win32::{
        Foundation::{
            COLORREF, CloseHandle, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, RECT,
            SetLastError, WIN32_ERROR, WPARAM,
        },
        Graphics::{
            Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute},
            Gdi::{
                BeginPaint, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
                COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateSolidBrush, DT_CENTER,
                DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawFocusRect, EndPaint, FillRect,
                FrameRect, GetSysColor, GetSysColorBrush, HBRUSH, HDC, HGDIOBJ, InvalidateRect,
                PAINTSTRUCT, SelectObject, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
                UpdateWindow,
            },
        },
        Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
            LibraryLoader::GetModuleHandleW,
            Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
            Threading::{CreateEventW, SetEvent},
        },
        UI::{
            Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, SetWindowTheme},
            HiDpi::{
                AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                GetDpiForWindow, GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_SHIFT},
            Shell::{
                Common::COMDLG_FILTERSPEC, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST,
                FOS_FORCEFILESYSTEM, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::{
                BS_OWNERDRAW, CREATESTRUCTW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW,
                GetWindowLongPtrW, GetWindowTextW, IDC_ARROW, IsWindowVisible, LoadCursorW, MSG,
                PostMessageW, PostQuitMessage, RegisterClassW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE,
                SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
                TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CAPTURECHANGED,
                WM_CLOSE, WM_COMMAND, WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX,
                WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_DRAWITEM, WM_ERASEBKGND,
                WM_GETOBJECT, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONUP, WM_MOUSEMOVE,
                WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCOLORCHANGE,
                WM_THEMECHANGED, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
            },
        },
    },
    core::{Error, PCWSTR, w},
};

use crate::{
    accessibility::{
        self, TabHit,
        native::{NativeAccessibility, SelectTab, is_uia_root_request},
        tab_slot, tabs_extent,
    },
    app::{AppState as PortableAppState, FocusSurface},
    documents::{
        self, RenderJob, RenderOutcome, RenderPool, RenderTickets, StartupSignal, StartupTracker,
    },
    keyboard::{KeyChord, ShellCommand, route_key},
    layout::{Dip, PixelRect, ShellLayout, ShellLayoutRequest},
    tabs::TabId,
    theme::{ColorSpec, EffectiveTheme, SystemColorRole, palette, resolve_theme},
};

// Shell chrome ported from the .NET MainWindow (toolbar, menu, sidebar,
// diagnostics bar, settings). Hooks into this file are marked "chrome:".
mod chrome;
mod diagnostics;
mod dialogs;
mod menu_theme;
mod settings_bridge;
mod shell_ops;
mod sidebar;
mod style;
mod toolbar;
// Find in page, drag and drop, single instance, tab close and reorder.
mod input;
// The WebView2 missing-runtime state and its user-initiated install. Hooks
// into this file are marked "runtime:".
mod runtime;
// Native adapters of the portable updater (`marknexia-update`). Hooks into
// this file are marked "update:".
mod update_native;

const CLASS_NAME: PCWSTR = w!("MarknexiaRustWindow");
const TAB_CLASS_NAME: PCWSTR = w!("MarknexiaRustTabStrip");
/// Applies a UIA-initiated selection to the WebView outside the UIA call.
const WM_APPLY_WEBVIEW_SELECTION: u32 = WM_APP + 1;
/// A render worker queued a result on the render channel.
const WM_RENDER_COMPLETE: u32 = WM_APP + 2;
/// Runs the Open dialog from the top-level loop, outside the button's
/// WM_COMMAND notification.
const WM_OPEN_DIALOG: u32 = WM_APP + 3;
const COMMAND_OPEN_ID: usize = 1001;
/// Worker stack for the recursive Markdown, math and sanitizer passes.
const RENDER_STACK_BYTES: usize = 8 * 1024 * 1024;
const WINDOW_TITLE: PCWSTR = w!("Marknexia");

#[derive(Debug)]
pub enum WindowError {
    Dpi(Error),
    Module(Error),
    Cursor(Error),
    Register(Error),
    Create(Error),
    Child(Error),
    State,
    MessageLoop(Error),
}

#[derive(Clone, Copy)]
struct ShellControls {
    command: HWND,
    tabs: HWND,
    sidebar: HWND,
    find: HWND,
    status: HWND,
}

struct AppState {
    portable: Rc<RefCell<PortableAppState>>,
    controls: Option<ShellControls>,
    webview: Option<WebViewSession>,
    apartment: Option<Rc<StaApartment>>,
    page_observer: Option<Rc<dyn Fn(PageToHost)>>,
    accessibility: Option<NativeAccessibility>,
    uia_selection: Option<Rc<SelectTab>>,
    /// True while the WebView session is taken out for a COM call. A
    /// reentrant selection must not report success while the active
    /// controller cannot be switched.
    webview_checked_out: bool,
    /// Tab whose controller the WebView session last made visible.
    webview_active: Option<u64>,
    readiness: Option<Readiness>,
    /// Dark-theme brushes for standard controls and the frame background;
    /// `None` in light and high-contrast themes (system colors apply).
    brushes: Option<ThemeBrushes>,
    /// Environment-level problem (WebView2 or COM unavailable, WebView2
    /// failure); shown when the active tab has nothing more specific.
    shell_error: Option<String>,
    /// Transient message (a failed selection or dialog); cleared when the
    /// active tab changes.
    notice: Option<String>,
    /// Per-document open/render failures, by tab.
    tab_errors: BTreeMap<u64, String>,
    dpi: u32,
    theme: EffectiveTheme,
    destroyed: bool,
    /// Renders in flight, by tab; stale results are discarded.
    renders: RenderTickets,
    /// Bounded worker pool, started with the first render.
    render_pool: Option<RenderPool>,
    render_sender: mpsc::Sender<RenderOutcome>,
    render_results: mpsc::Receiver<RenderOutcome>,
    /// Results arrived while the session was checked out; the message loop
    /// applies them after the outer call returns.
    render_results_deferred: bool,
    /// Decides FirstRender versus StartupFailed (exactly one, once).
    startup: StartupTracker,
    /// Tabs whose rendered Markdown document is in the WebView session.
    rendered_tabs: BTreeSet<u64>,
    /// chrome: toolbar, sidebar, diagnostics bar and settings.
    chrome: chrome::ChromeState,
    /// Find, drag and drop, activation and tab-pointer state (`input`).
    input: input::InputState,
    /// runtime: the WebView2 missing-runtime state.
    runtime: runtime::RuntimeUi,
    /// The start-page tab (no document), re-rendered on theme changes.
    welcome_tab: Option<u64>,
}

impl AppState {
    fn new() -> Self {
        let (render_sender, render_results) = mpsc::channel();
        Self {
            renders: RenderTickets::new(),
            render_pool: None,
            render_results_deferred: false,
            startup: StartupTracker::default(),
            render_sender,
            render_results,
            rendered_tabs: BTreeSet::new(),
            chrome: chrome::ChromeState::new(),
            input: input::InputState::default(),
            runtime: runtime::RuntimeUi::new(),
            welcome_tab: None,
            portable: Rc::new(RefCell::new(PortableAppState::new())),
            controls: None,
            webview: None,
            apartment: None,
            page_observer: None,
            accessibility: None,
            uia_selection: None,
            webview_checked_out: false,
            webview_active: None,
            readiness: None,
            brushes: None,
            shell_error: None,
            notice: None,
            tab_errors: BTreeMap::new(),
            dpi: 96,
            theme: EffectiveTheme::Light,
            destroyed: false,
        }
    }
}

struct CreatePayload {
    // GWLP_USERDATA owns exactly one boxed strong handle, so the slot has a
    // stable address independent of the Rc allocation.
    #[allow(clippy::redundant_allocation)]
    state: Option<Box<Rc<RefCell<AppState>>>>,
}

pub fn run() -> Result<(), WindowError> {
    let arguments: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // update: `--post-update-health <nonce>` starts a new version for its
    // health check: no documents, no single-instance hand-off (the old
    // version is still running), and an acknowledgement once ready.
    let health = marknexia_update::activation::health_nonce_from_arguments(&arguments);
    // update: a normal start first resolves an interrupted update, then
    // runs a newer active portable version instead of this one.
    if health.is_none() && update_native::forward_to_active_version(&arguments) {
        return Ok(());
    }
    // Single instance (.NET `OnLaunched`): a later launch hands its command
    // line to the running instance and exits.
    let primary = if health.is_some() {
        None
    } else {
        match input::claim_instance() {
            crate::single_instance::native::Claim::HandedOff => return Ok(()),
            crate::single_instance::native::Claim::Primary(primary) => Some(primary),
            crate::single_instance::native::Claim::Standalone => None,
        }
    };
    let paths = if health.is_some() {
        Vec::new()
    } else {
        documents::startup_paths(arguments)
    };
    enable_dpi().map_err(WindowError::Dpi)?;
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(WindowError::Module)?;
    let instance = HINSTANCE(module.0);
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(WindowError::Cursor)?;
    for class in [
        WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: CLASS_NAME,
            hCursor: cursor,
            hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
            ..Default::default()
        },
        WNDCLASSW {
            lpfnWndProc: Some(tab_proc),
            hInstance: instance,
            lpszClassName: TAB_CLASS_NAME,
            hCursor: cursor,
            hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
            ..Default::default()
        },
    ] {
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(WindowError::Register(Error::from_thread()));
        }
    }
    let mut payload = CreatePayload {
        state: Some(Box::new(Rc::new(RefCell::new(AppState::new())))),
    };
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            WINDOW_TITLE,
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            760,
            None,
            None,
            Some(instance),
            Some((&mut payload as *mut CreatePayload).cast::<c_void>()),
        )
    }
    .map_err(WindowError::Create)?;
    size_for_dpi(hwnd);
    if let Err(error) = initialize(hwnd, instance, paths, health) {
        let _ = unsafe { DestroyWindow(hwnd) };
        return Err(error);
    }
    input::serve_activations(hwnd, primary);
    let mut msg = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if status.0 == -1 {
            let error = Error::from_thread();
            let _ = unsafe { DestroyWindow(hwnd) };
            return Err(WindowError::MessageLoop(error));
        }
        if status.0 == 0 {
            break;
        }
        let handled = msg.message == WM_KEYDOWN
            && (input::route_key(hwnd, msg.hwnd, msg.wParam.0 as u16)
                || route_native_key(hwnd, msg.wParam.0 as u16));
        // chrome: .NET accelerators, Tab traversal, list activation.
        let handled = handled || chrome::translate_message(hwnd, &msg);
        if !handled {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        poll_webview(hwnd);
        // Work that arrived while an outer frame held the state (input).
        input::run_deferred(hwnd);
        // Results that arrived while an outer call held the session.
        apply_deferred_render_results(hwnd);
        // runtime: WebView2 setup progress and results.
        runtime::poll(hwnd);
        // Work still queued (its wake-up consumed by a pump inside the tail
        // work) gets exactly one new wake-up, so the loop never blocks.
        input::rearm_wake();
    }
    Ok(())
}

/// Sizes the new, still hidden window for its monitor's DPI: a client area
/// of 1100 × 760 DIPs (with the menu bar), clamped to the work area, so a
/// high-DPI monitor does not start in the compact layout.
fn size_for_dpi(hwnd: HWND) {
    use windows::Win32::{
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::{HiDpi::AdjustWindowRectExForDpi, WindowsAndMessaging::SWP_NOMOVE},
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let mut frame = RECT::default();
    // The menu bar is attached later by `chrome::install`; count it now.
    if unsafe {
        AdjustWindowRectExForDpi(
            &mut frame,
            WS_OVERLAPPEDWINDOW,
            true,
            WINDOW_EX_STYLE::default(),
            dpi,
        )
    }
    .is_err()
    {
        return;
    }
    let mut monitor = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let work = if unsafe {
        GetMonitorInfoW(
            MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        )
    }
    .as_bool()
    {
        let area = monitor.rcWork;
        (
            u32::try_from(area.right - area.left).unwrap_or(u32::MAX),
            u32::try_from(area.bottom - area.top).unwrap_or(u32::MAX),
        )
    } else {
        (u32::MAX, u32::MAX)
    };
    let (width, height) = crate::layout::initial_window_size(
        dpi,
        (
            u32::try_from(frame.right - frame.left).unwrap_or(0),
            u32::try_from(frame.bottom - frame.top).unwrap_or(0),
        ),
        work,
    );
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            i32::try_from(width).unwrap_or(i32::MAX),
            i32::try_from(height).unwrap_or(i32::MAX),
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

fn child_style(extra: u32) -> WINDOW_STYLE {
    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | extra)
}

fn create_child(
    parent: HWND,
    instance: HINSTANCE,
    class: PCWSTR,
    title: PCWSTR,
    style: WINDOW_STYLE,
    id: isize,
) -> Result<HWND, WindowError> {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            title,
            style,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(windows::Win32::UI::WindowsAndMessaging::HMENU(
                id as *mut c_void,
            )),
            Some(instance),
            None,
        )
    }
    .map_err(WindowError::Child)
}

fn initialize(
    hwnd: HWND,
    instance: HINSTANCE,
    paths: Vec<PathBuf>,
    health: Option<String>,
) -> Result<(), WindowError> {
    let controls = ShellControls {
        command: create_child(
            hwnd,
            instance,
            w!("BUTTON"),
            // .NET `OpenFileButton` automation name.
            w!("Open Markdown file"),
            // Owner-drawn: themed push buttons ignore documented dark styles.
            child_style(BS_OWNERDRAW as u32),
            1001,
        )?,
        tabs: create_child(hwnd, instance, TAB_CLASS_NAME, w!(""), child_style(0), 1002)?,
        // chrome: the sidebar view selector (.NET `SidebarModeSelector`).
        sidebar: sidebar::create_mode_selector(hwnd, instance)?,
        find: create_child(
            hwnd,
            instance,
            w!("EDIT"),
            w!("Find"),
            // ES_AUTOHSCROLL and a one-pixel border (themed).
            child_style(0x80 | 0x0080_0000),
            1004,
        )?,
        status: create_child(
            hwnd,
            instance,
            w!("STATIC"),
            w!("Ready"),
            // SS_NOPREFIX | SS_CENTERIMAGE | SS_ENDELLIPSIS: one centred line.
            child_style(0x80 | 0x200 | 0x4000),
            1005,
        )?,
    };
    let state = unsafe { app_state_handle(hwnd) }.ok_or(WindowError::State)?;
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let portable = {
        let mut state = state.borrow_mut();
        state.controls = Some(controls);
        state.dpi = dpi;
        state.readiness = Some(Readiness::create(health));
        Rc::clone(&state.portable)
    };
    // chrome: controls, menu and saved settings, before startup renders.
    chrome::install(hwnd, instance, &state)?;
    let close_callback = input::initialize(hwnd, instance, &state, controls.find)?;
    // Placeholder tabs only without documents. Document renders start before
    // the window is shown and overlap WebView2 startup.
    // Document mode follows the requested paths, not the tabs that opened:
    // paths that could not get a tab still mean the startup failed.
    let document_mode = !paths.is_empty();
    let startup_documents = {
        let mut app = portable.borrow_mut();
        if paths.is_empty() {
            // The start page (.NET `WelcomePanel`), one tab, rendered like
            // a document below.
            let _ = app.open_tab(documents::WELCOME_TITLE);
            Vec::new()
        } else {
            paths
                .into_iter()
                .filter_map(|path| {
                    app.open_tab(documents::tab_title(&path))
                        .ok()
                        .map(|id| (id, path))
                })
                .collect::<Vec<_>>()
        }
    };
    {
        // Startup tabs: the document tabs, or the active placeholder.
        let startup_tabs: Vec<u64> = if document_mode {
            startup_documents.iter().map(|(id, _)| id.get()).collect()
        } else {
            portable
                .borrow()
                .active_tab()
                .map(TabId::get)
                .into_iter()
                .collect()
        };
        let mut state = state.borrow_mut();
        state.startup = StartupTracker::new(document_mode, startup_tabs);
    }
    for (tab_id, path) in startup_documents {
        start_render(hwnd, &state, tab_id.get(), path);
    }
    if !document_mode {
        // The start page renders through the document pipeline, so it
        // has the themed template; it stays the startup tab.
        let welcome = portable.borrow().active_tab();
        if let Some(welcome) = welcome {
            state.borrow_mut().welcome_tab = Some(welcome.get());
            render_welcome(hwnd, &state, welcome.get());
        }
    }
    // Every startup render may already have failed to queue.
    update_startup_readiness(&state);
    // UIA delivers `Select` inside an input-synchronous cross-process call,
    // where WebView2 rejects outgoing COM (0x802A000C). Acknowledgement
    // contract: on return, portable selection, the repainted tab strip, and
    // UIA state agree; the WebView switch is posted and runs on the next loop
    // turn. If that switch fails, selection reverts to the tab the WebView
    // still shows and UIA observers get the matching events.
    let select_callback: Rc<SelectTab> =
        Rc::new(move |tab_id: TabId| select_from_automation(hwnd, tab_id));
    let accessibility = NativeAccessibility::new(
        controls.tabs,
        Rc::downgrade(&portable),
        Rc::downgrade(&select_callback),
        Rc::downgrade(&close_callback),
    );
    {
        let mut state = state.borrow_mut();
        state.accessibility = Some(accessibility);
        state.uia_selection = Some(select_callback);
    }
    // Lay out and paint the native shell before WebView2 startup so the
    // window appears without waiting for the browser process.
    update_theme(hwnd);
    reflow(hwnd, dpi);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
    }
    // update: the health check's "shell visible" probe.
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    if let Some(readiness) = state.borrow_mut().readiness.as_mut() {
        readiness.shell_shown(visible);
    }
    match StaApartment::enter() {
        Ok(apartment) => {
            let apartment = Rc::new(apartment);
            // runtime: without an Evergreen runtime no session is created;
            // the shell shows the missing-runtime state instead.
            if runtime::runtime_missing() {
                // The STA stays entered: the Open dialog and UIA need COM.
                state.borrow_mut().apartment = Some(apartment);
                runtime::show_missing(hwnd, &state);
            } else {
                start_session(hwnd, &state, apartment);
            }
        }
        Err(error) => {
            let mut state = state.borrow_mut();
            state.shell_error = Some(format!("COM STA unavailable: {error:?}"));
            let signal = state.startup.webview_unavailable();
            state.apply_startup(signal);
        }
    }
    // Apply theme and bounds to the WebView session created above.
    update_theme(hwnd);
    reflow(hwnd, dpi);
    // With document paths and no WebView2 nothing can render: signal
    // StartupFailed now so a harness does not wait for its timeout.
    update_startup_readiness(&state);
    refresh_status(&state);
    update_window_title(&state);
    // chrome: restore the saved repository, show the chrome state.
    chrome::after_startup(hwnd, &state, document_mode);
    Ok(())
}

/// runtime: WebView2 was installed from the missing-runtime state. The
/// session starts now; documents rendered while it was missing were not
/// kept, so they render again and join the session when they arrive.
fn start_after_install(hwnd: HWND) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    let apartment = {
        let state = state.borrow();
        if state.destroyed || state.webview.is_some() || state.webview_checked_out {
            return;
        }
        state.apartment.clone()
    };
    let Some(apartment) = apartment else {
        return;
    };
    state.borrow_mut().shell_error = None;
    let documents = state.borrow().chrome.document_tabs();
    for (tab_id, path) in documents {
        state.borrow_mut().tab_errors.remove(&tab_id);
        start_render(hwnd, &state, tab_id, path);
    }
    start_session(hwnd, &state, apartment);
    let dpi = state.borrow().dpi;
    update_theme(hwnd);
    reflow(hwnd, dpi);
    refresh_status(&state);
}

/// Creates the WebView session with every tab that is not waiting for its
/// render (placeholders get their text document), shows the active tab and
/// starts environment creation. A failed start keeps the shell usable and
/// reports WebView2 as unavailable.
fn start_session(hwnd: HWND, state: &Rc<RefCell<AppState>>, apartment: Rc<StaApartment>) {
    let portable = Rc::clone(&state.borrow().portable);
    // Until the session is stored, a reentrant UIA Select must not
    // report success against a controller it cannot reach.
    state.borrow_mut().webview_checked_out = true;
    let observer: Rc<dyn Fn(PageToHost)> = input::page_observer(state);
    let mut session = WebViewSession::new(
        Rc::clone(&apartment),
        hwnd,
        webview_folder(),
        Rc::downgrade(&observer),
    );
    let (tabs, active) = {
        let app = portable.borrow();
        (
            app.tabs()
                .tabs()
                .iter()
                .map(|tab| (tab.id(), tab.title().to_owned()))
                .collect::<Vec<_>>(),
            app.active_tab(),
        )
    };
    let mut startup_error = None;
    // Placeholders that could not join the session (or be shown):
    // as startup tabs they resolve as failed, never stay pending.
    let mut failed_placeholders = Vec::new();
    for (tab_id, title) in tabs {
        // Document tabs join the session when their render arrives.
        if state.borrow().renders.is_pending(tab_id.get()) {
            continue;
        }
        // A tab without a pending render (the start page after a
        // WebView2 install): its shell page, on the themed template.
        let markdown = if state.borrow().welcome_tab == Some(tab_id.get()) {
            documents::WELCOME_MARKDOWN.to_owned()
        } else {
            documents::markdown_text(&title)
        };
        let theme = app_theme(state);
        let document = text_document(tab_id.get(), &title, &markdown, theme);
        let added = document.and_then(|document| {
            session
                .add_document(document)
                .map_err(|error| format!("{error:?}"))
        });
        if let Err(error) = added {
            startup_error = Some(format!("WebView2 document: {error}"));
            failed_placeholders.push(tab_id.get());
        }
    }
    let mut shown = None;
    if let Some(active) = active {
        if session.select_tab(active.get()).is_ok() {
            shown = Some(active.get());
        } else if !state.borrow().renders.is_pending(active.get()) {
            // A placeholder that cannot be shown; a document tab
            // whose render is pending is expected to be unselectable.
            failed_placeholders.push(active.get());
        }
    }
    {
        let mut state = state.borrow_mut();
        for tab_id in failed_placeholders {
            let signal = state.startup.failed(tab_id);
            state.apply_startup(signal);
        }
    }
    match session.start() {
        Ok(()) => {
            let mut state = state.borrow_mut();
            state.webview_checked_out = false;
            state.webview_active = shown;
            state.webview = Some(session);
            state.apartment = Some(apartment);
            state.page_observer = Some(observer);
            state.shell_error = startup_error;
        }
        Err(error) => {
            let _ = session.close();
            drop(session);
            let mut state = state.borrow_mut();
            state.webview_checked_out = false;
            // The STA stays entered without WebView2: the Open dialog
            // and UIA still need COM on this thread.
            state.apartment = Some(apartment);
            state.shell_error = Some(format!("WebView2 unavailable: {error:?}"));
            let signal = state.startup.webview_unavailable();
            state.apply_startup(signal);
        }
    }
}

/// A page the shell writes itself (the start page, an error message) from
/// Markdown, through the document renderer: the themed template, its
/// stylesheet, the sanitizer and the page policy, so no shell page ever
/// renders unstyled. Text from elsewhere must be escaped with
/// `documents::markdown_text` first.
fn text_document(
    tab_id: u64,
    title: &str,
    markdown: &str,
    theme: AppTheme,
) -> Result<HostDocument, String> {
    documents::build_shell_page(tab_id, title, markdown, theme, page_identity)
        .map_err(|error| format!("{error}"))
}

/// The document theme for the current preference.
fn app_theme(state: &Rc<RefCell<AppState>>) -> AppTheme {
    let preference = crate::reentry::read(state, |state| {
        crate::reentry::read(&state.portable, |app| app.theme())
    })
    .flatten()
    .unwrap_or(crate::theme::ThemePreference::System);
    match preference {
        crate::theme::ThemePreference::System => AppTheme::System,
        crate::theme::ThemePreference::Light => AppTheme::Light,
        crate::theme::ThemePreference::Dark => AppTheme::Dark,
    }
}

/// Renders (or re-renders, after a theme change) the start page in `tab`
/// on the render pool, like a document but from built-in Markdown: no
/// file, no recent-file entry, no repository detection.
fn render_welcome(hwnd: HWND, state: &Rc<RefCell<AppState>>, tab: u64) {
    queue_render(
        hwnd,
        state,
        tab,
        PathBuf::new(),
        Some(documents::WELCOME_MARKDOWN.to_owned()),
    );
}

/// chrome: after a theme change, the start page follows the new theme
/// (documents reload through `reload_active`).
fn rerender_shell_pages(hwnd: HWND) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    let welcome = crate::reentry::read(&state, |state| state.welcome_tab).flatten();
    let open = welcome.is_some_and(|tab| {
        crate::reentry::read(&state, |state| {
            crate::reentry::read(&state.portable, |app| {
                app.tabs()
                    .tabs()
                    .iter()
                    .any(|candidate| candidate.id().get() == tab)
            })
        })
        .flatten()
        .unwrap_or(false)
    });
    match welcome {
        Some(tab) if open => render_welcome(hwnd, &state, tab),
        _ => {
            let _ = crate::reentry::write(&state, |state| state.welcome_tab = None);
        }
    }
}

/// A fresh per-document page identity from the OS RNG. `PageIdentity`
/// rejects all-zero values; a zero draw (never expected) is retried.
fn page_identity() -> Result<PageIdentity, String> {
    for _ in 0..4 {
        let mut bytes = [0_u8; 32];
        // SAFETY: the buffer is a live local; the system-preferred RNG needs
        // no algorithm handle and is callable from any thread.
        unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
            .ok()
            .map_err(|error| format!("BCryptGenRandom failed: {error}"))?;
        let mut document_id = [0_u8; 16];
        let mut nonce = [0_u8; 16];
        document_id.copy_from_slice(&bytes[..16]);
        nonce.copy_from_slice(&bytes[16..]);
        if let Ok(identity) = PageIdentity::new(document_id, nonce) {
            return Ok(identity);
        }
    }
    Err("the OS RNG returned zero bytes".to_owned())
}

/// Queues `path` on the bounded render pool. Workers own only `Send` data:
/// they read, render and build the `HostDocument` off the UI thread, never
/// touch HWND state, `AppState` or WebView2, and wake the UI thread with
/// `WM_RENDER_COMPLETE` after queueing the result.
fn start_render(hwnd: HWND, state: &Rc<RefCell<AppState>>, tab_id: u64, path: PathBuf) {
    queue_render(hwnd, state, tab_id, path, None);
}

/// `start_render`, or with `builtin` Markdown (the start page) instead of
/// the file at `path`.
fn queue_render(
    hwnd: HWND,
    state: &Rc<RefCell<AppState>>,
    tab_id: u64,
    path: PathBuf,
    builtin: Option<String>,
) {
    let theme = app_theme(state);
    let mut guard = state.borrow_mut();
    let state_ref = &mut *guard;
    let title = if builtin.is_some() {
        documents::WELCOME_TITLE.to_owned()
    } else {
        documents::tab_title(&path)
    };
    let (ticket, cancelled) = state_ref.renders.begin(tab_id);
    state_ref.tab_errors.remove(&tab_id);
    if builtin.is_none() {
        // chrome: tab location, recent files, repository root detection.
        state_ref.chrome.note_render(tab_id, &path);
    }
    let allow_remote_assets = state_ref.chrome.allow_remote_assets();
    let render_text_diagrams = state_ref.chrome.render_text_diagrams();
    let pool = state_ref.render_pool.get_or_insert_with(|| {
        let window = hwnd.0 as isize;
        let notify: documents::Notify = Arc::new(move || {
            // SAFETY: PostMessageW may target a window owned by another
            // thread; after the window is destroyed the post just fails.
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(window as *mut c_void)),
                    WM_RENDER_COMPLETE,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
        });
        RenderPool::new(
            RenderPool::default_workers(),
            RENDER_STACK_BYTES,
            page_identity,
            state_ref.render_sender.clone(),
            notify,
        )
    });
    let submitted = pool.submit(RenderJob {
        tab_id,
        ticket,
        path,
        title,
        theme,
        allow_remote_assets,
        render_text_diagrams,
        builtin,
        cancelled,
    });
    if let Err(error) = submitted {
        state_ref.renders.cancel(tab_id);
        state_ref
            .tab_errors
            .insert(tab_id, format!("Could not start rendering: {error}"));
        let signal = state_ref.startup.failed(tab_id);
        state_ref.apply_startup(signal);
    }
}

/// Applies queued render results on the UI thread: results for closed tabs
/// are dropped, others join the WebView session.
fn apply_render_results(hwnd: HWND) {
    // SAFETY: `hwnd` is this thread's shell window; the handle helper returns
    // a scoped strong reference and no borrow.
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    if !shell_free(&state) {
        // Dispatched by a nested pump while an outer frame holds the state.
        input::defer_render_results();
        return;
    }
    if state.borrow().webview_checked_out {
        // An outer call holds the WebView session. The message loop applies
        // the results once that call returns; nothing is re-posted.
        state.borrow_mut().render_results_deferred = true;
        return;
    }
    state.borrow_mut().render_results_deferred = false;
    let outcomes: Vec<RenderOutcome> = state.borrow().render_results.try_iter().collect();
    for outcome in outcomes {
        apply_render_outcome(&state, outcome);
    }
    update_startup_readiness(&state);
    refresh_status(&state);
}

fn apply_deferred_render_results(hwnd: HWND) {
    // SAFETY: as in `apply_render_results`.
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    let ready = {
        let state = state.borrow();
        state.render_results_deferred && !state.webview_checked_out
    };
    if ready {
        apply_render_results(hwnd);
    }
}

fn apply_render_outcome(state: &Rc<RefCell<AppState>>, outcome: RenderOutcome) {
    let RenderOutcome {
        tab_id,
        ticket,
        result,
        outline,
    } = outcome;
    let portable = Rc::clone(&state.borrow().portable);
    let title = portable
        .borrow()
        .tabs()
        .tabs()
        .iter()
        .find(|tab| tab.id().get() == tab_id)
        .map(|tab| tab.title().to_owned());
    let accepted = state.borrow_mut().renders.complete(tab_id, ticket);
    let (Some(title), true) = (title, accepted) else {
        // The tab closed (or re-rendered) while this render was in flight.
        return;
    };
    // chrome: outline, diagnostics; a failed reload keeps its document.
    let Some(result) = chrome::filter_render_outcome(state, tab_id, result, outline) else {
        return;
    };
    let rendered = result.is_ok();
    let theme = app_theme(state);
    let document = result.or_else(|error| {
        let message = format!("Could not open {title}: {error}");
        state
            .borrow_mut()
            .tab_errors
            .insert(tab_id, message.clone());
        text_document(tab_id, &title, &documents::markdown_text(&message), theme)
    });
    let mut displayed = true;
    input::document_replaced(state, tab_id);
    let added = match take_session(state) {
        Some(mut session) => {
            // chrome: a reload replaces the tab's earlier document.
            if session.has_document(tab_id) {
                let _ = session.remove_document(tab_id);
            }
            let added = document.and_then(|document| {
                session
                    .add_document(document)
                    .map_err(|error| format!("WebView2 document: {error:?}"))
            });
            restore_session(state, session);
            added
        }
        // Without WebView2 nothing can be displayed; the shell error says so,
        // and the document does not count as rendered.
        None => {
            displayed = false;
            document.map(drop)
        }
    };
    {
        let mut state = state.borrow_mut();
        match added {
            // A startup tab stays pending until its navigation completes (or
            // its controller or navigation fails): only then is it decided.
            Ok(()) if rendered && displayed => {
                state.rendered_tabs.insert(tab_id);
            }
            Ok(()) => {
                let signal = state.startup.failed(tab_id);
                state.apply_startup(signal);
            }
            Err(error) => {
                state.tab_errors.insert(tab_id, error);
                let signal = state.startup.failed(tab_id);
                state.apply_startup(signal);
            }
        }
    }
    let active = portable.borrow().active_tab().map(TabId::get);
    if active == Some(tab_id) {
        let _ = select_webview(state, tab_id);
    }
}

/// Re-evaluates startup readiness: with document paths, `StartupFailed`
/// once nothing is pending and nothing rendered, so a harness fails fast
/// instead of waiting for a `FirstRender` that cannot come.
fn update_startup_readiness(state: &Rc<RefCell<AppState>>) {
    let mut state = state.borrow_mut();
    let signal = state.startup.evaluate();
    state.apply_startup(signal);
}

impl AppState {
    /// Raises the readiness event for a startup decision, if any.
    fn apply_startup(&mut self, signal: Option<StartupSignal>) {
        if let (Some(signal), Some(readiness)) = (signal, self.readiness.as_mut()) {
            readiness.signal(signal);
        }
    }
}

fn open_from_dialog(hwnd: HWND) {
    // WM_OPEN_DIALOG dispatched by a nested pump while an outer frame holds
    // the state: open the dialog from the message loop instead.
    if unsafe { app_state_handle(hwnd) }.is_some_and(|state| !shell_free(&state)) {
        input::defer_open_dialog();
        return;
    }
    // No AppState borrow is held: the dialog runs a nested modal loop.
    match show_open_dialog(hwnd) {
        Ok(paths) => {
            for path in paths {
                open_document(hwnd, path);
            }
        }
        Err(error) => {
            // SAFETY: `hwnd` is this thread's shell window; see
            // `app_state_handle`.
            if let Some(state) = unsafe { app_state_handle(hwnd) } {
                state.borrow_mut().notice = Some(format!("Open dialog failed: {error}"));
                refresh_status(&state);
            }
        }
    }
}

/// Opens `path` in a new active tab and starts rendering it.
fn open_document(hwnd: HWND, path: PathBuf) {
    // SAFETY: `hwnd` is this thread's shell window; see `app_state_handle`.
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    // .NET: an already open document selects its tab.
    if input::select_open_document(hwnd, &state, &path) {
        return;
    }
    let portable = Rc::clone(&state.borrow().portable);
    let opened = portable.borrow_mut().open_tab(documents::tab_title(&path));
    let Ok(tab_id) = opened else {
        state.borrow_mut().notice = Some("No more tabs can be opened.".to_owned());
        refresh_status(&state);
        return;
    };
    start_render(hwnd, &state, tab_id.get(), path);
    // The new tab has no document yet: hide the previous controller.
    let _ = select_webview(&state, tab_id.get());
    let a11y = state.borrow().accessibility.clone();
    if let Some(a11y) = a11y {
        a11y.notify_children_changed();
    }
    refresh_tabs(&state, Some(tab_id));
}

/// The common Open dialog for Markdown and text files. Cancel returns no
/// paths.
fn show_open_dialog(owner: HWND) -> Result<Vec<PathBuf>, Error> {
    const CANCELLED: i32 = 0x8007_04C7_u32 as i32;
    // SAFETY: runs on the shell's UI thread. When `StaApartment::enter`
    // failed at startup COM is not initialized here, and CoCreateInstance
    // returns CO_E_NOTINITIALIZED, which is reported in the status bar. The
    // apartment is otherwise kept for the window's lifetime, with or without
    // WebView2. Interfaces are released on drop, the filter strings are
    // static, and every returned display name is freed with CoTaskMemFree.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: w!("Markdown and text (*.md; *.markdown; *.txt)"),
                pszSpec: w!("*.md;*.markdown;*.txt"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("Markdown (*.md; *.markdown)"),
                pszSpec: w!("*.md;*.markdown"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("Text (*.txt)"),
                pszSpec: w!("*.txt"),
            },
        ];
        dialog.SetFileTypes(&filters)?;
        dialog.SetFileTypeIndex(1)?;
        let options = dialog.GetOptions()?;
        dialog
            .SetOptions(options | FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST | FOS_ALLOWMULTISELECT)?;
        if let Err(error) = dialog.Show(Some(owner)) {
            return if error.code().0 == CANCELLED {
                Ok(Vec::new())
            } else {
                Err(error)
            };
        }
        let items = dialog.GetResults()?;
        let mut paths = Vec::new();
        for index in 0..items.GetCount()? {
            let name = items.GetItemAt(index)?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let text = name.to_string();
            CoTaskMemFree(Some(name.0 as *const c_void));
            if let Ok(text) = text {
                paths.push(PathBuf::from(text));
            }
        }
        Ok(paths)
    }
}

/// The status of what the user is looking at, most specific first: the
/// active document's failure, "Rendering…" while its render is pending, a
/// transient notice, an environment problem, or "Ready". The shell stays
/// usable without WebView2; the status then explains why documents are not
/// displayed.
fn refresh_status(state: &Rc<RefCell<AppState>>) {
    let (status, text) = {
        // Reentrant callers (UIA, nested pumps) skip the refresh while an
        // outer frame holds the state; the next refresh shows it.
        let Ok(state) = state.try_borrow() else {
            return;
        };
        let Ok(app) = state.portable.try_borrow() else {
            return;
        };
        let active = app.active_tab();
        let title = active.and_then(|tab| {
            app.tabs()
                .tabs()
                .iter()
                .find(|candidate| candidate.id() == tab)
                .map(|candidate| candidate.title().to_owned())
        });
        let tab_error = active.and_then(|tab| state.tab_errors.get(&tab.get()));
        let pending = active.is_some_and(|tab| state.renders.is_pending(tab.get()));
        let text = if let Some(error) = tab_error {
            error.clone()
        } else if let (true, Some(title)) = (pending, title) {
            format!("Rendering {title}\u{2026}")
        } else if let Some(notice) = state.notice.as_deref() {
            notice.to_owned()
        } else {
            match state.shell_error.as_deref() {
                None => "Ready".to_owned(),
                Some(error) if error.starts_with("WebView2 unavailable") => {
                    "Microsoft Edge WebView2 Runtime is not available; documents cannot be displayed."
                        .to_owned()
                }
                Some(error) => error.to_owned(),
            }
        };
        (state.controls.map(|c| c.status), text)
    };
    if let Some(status) = status {
        let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `text` is NUL-terminated and outlives the call; the STATIC
        // control belongs to this thread and no AppState borrow is held.
        let _ = unsafe { SetWindowTextW(status, PCWSTR::from_raw(text.as_ptr())) };
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // chrome: control notifications, owner-drawn chrome, chrome actions.
    if message != WM_NCCREATE
        && let Some(result) = chrome::handle_message(hwnd, message, wparam, lparam)
    {
        return result;
    }
    // runtime: the install button, the offline link and setup wake-ups.
    if message != WM_NCCREATE
        && let Some(result) = runtime::handle_message(hwnd, message, wparam, lparam)
    {
        return result;
    }
    match message {
        WM_NCCREATE => {
            let creation = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            let payload = unsafe { &mut *(creation.lpCreateParams as *mut CreatePayload) };
            let Some(state) = payload.state.take() else {
                return LRESULT(0);
            };
            let state = Box::into_raw(state);
            if unsafe { set_user_data(hwnd, state as isize) }.is_err() {
                payload.state = Some(unsafe { Box::from_raw(state) });
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_DPICHANGED => {
            if let Some(rect) = unsafe { (lparam.0 as *const RECT).as_ref() } {
                let _ = unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        rect.left,
                        rect.top,
                        rect.right.saturating_sub(rect.left),
                        rect.bottom.saturating_sub(rect.top),
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                };
            }
            reflow(hwnd, (wparam.0 & 0xffff) as u32);
            LRESULT(0)
        }
        WM_SIZE => {
            reflow(hwnd, unsafe { GetDpiForWindow(hwnd) }.max(96));
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
            update_theme(hwnd);
            reflow(hwnd, unsafe { GetDpiForWindow(hwnd) }.max(96));
            LRESULT(0)
        }
        WM_KEYDOWN if route_native_key(hwnd, wparam.0 as u16) => LRESULT(0),
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
            match themed_control_brush(hwnd, HDC(wparam.0 as *mut c_void)) {
                Some(brush) => LRESULT(brush.0 as isize),
                None => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
            }
        }
        WM_DRAWITEM => {
            // SAFETY: for WM_DRAWITEM, lParam points to a DRAWITEMSTRUCT that
            // is valid for the duration of this message.
            match unsafe { (lparam.0 as *const DRAWITEMSTRUCT).as_ref() } {
                Some(item)
                    if draw_command_button(hwnd, item) || input::draw_find_button(hwnd, item) =>
                {
                    LRESULT(1)
                }
                _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
            }
        }
        WM_ERASEBKGND => {
            if erase_themed_background(hwnd, HDC(wparam.0 as *mut c_void)) {
                LRESULT(1)
            } else {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        WM_APPLY_WEBVIEW_SELECTION => {
            apply_automation_selection(hwnd, wparam.0 as u64);
            LRESULT(0)
        }
        // BN_CLICKED (notification code 0) from the Open Markdown file button.
        WM_COMMAND if wparam.0 & 0xffff == COMMAND_OPEN_ID && (wparam.0 >> 16) & 0xffff == 0 => {
            // SAFETY: posts to this thread's own live window; no pointers.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_OPEN_DIALOG, WPARAM(0), LPARAM(0)) };
            LRESULT(0)
        }
        WM_COMMAND if input::on_command(hwnd, wparam) => LRESULT(0),
        WM_OPEN_DIALOG => {
            open_from_dialog(hwnd);
            LRESULT(0)
        }
        message if input::handles(message) => input::on_message(hwnd, message, wparam, lparam),
        WM_RENDER_COMPLETE => {
            apply_render_results(hwnd);
            LRESULT(0)
        }
        // WM_CLOSE dispatched by a nested pump while an outer frame holds
        // the state: close from the message loop (WM_DESTROY needs it).
        WM_CLOSE if unsafe { app_state_handle(hwnd) }.is_some_and(|state| !shell_free(&state)) => {
            input::defer_close_window();
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(state) = unsafe { app_state_handle(hwnd) } {
                close_state(&state);
            }
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
            let cleared = raw == 0
                || (unsafe { set_user_data(hwnd, 0) }.is_ok()
                    && unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } == 0);
            let owner = if raw != 0 && cleared {
                Some(unsafe { Box::from_raw(raw as *mut Rc<RefCell<AppState>>) })
            } else {
                None
            };
            if let Some(state) = owner.as_ref() {
                close_state(state);
            }
            let result = unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            drop(owner);
            result
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe extern "system" fn tab_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_GETOBJECT if is_uia_root_request(lparam) => {
            let Ok(parent) = (unsafe { windows::Win32::UI::WindowsAndMessaging::GetParent(hwnd) })
            else {
                return LRESULT(0);
            };
            let Some(state) = (unsafe { app_state_handle(parent) }) else {
                return LRESULT(0);
            };
            // UIA sends WM_GETOBJECT synchronously, also into nested pumps.
            let provider =
                crate::reentry::read(&state, |state| state.accessibility.clone()).flatten();
            provider.as_ref().map_or(LRESULT(0), |provider| unsafe {
                provider.return_provider(wparam, lparam)
            })
        }
        // Select, close-button press, drag reorder (`input`).
        WM_LBUTTONDOWN => {
            input::on_tab_button_down(hwnd, (lparam.0 as i16) as i32);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            track_tab_hover(hwnd, Some((lparam.0 as i16) as i32));
            input::on_tab_mouse_move(hwnd, (lparam.0 as i16) as i32, wparam.0);
            LRESULT(0)
        }
        // WM_MOUSELEAVE: the hover look ends.
        0x02A3 => {
            track_tab_hover(hwnd, None);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            input::on_tab_button_up(hwnd, (lparam.0 as i16) as i32);
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            input::on_tab_capture_lost(hwnd);
            LRESULT(0)
        }
        WM_MBUTTONUP => {
            input::on_tab_middle_click(hwnd, (lparam.0 as i16) as i32);
            LRESULT(0)
        }
        WM_PAINT => {
            paint_tabs(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn paint_tabs(hwnd: HWND) {
    let Ok(parent) = (unsafe { windows::Win32::UI::WindowsAndMessaging::GetParent(hwnd) }) else {
        return;
    };
    let Some(state) = (unsafe { app_state_handle(parent) }) else {
        return;
    };
    let free = shell_free(&state)
        && crate::reentry::read(&state, |state| crate::reentry::is_free(&state.portable))
            .unwrap_or(false);
    if !free {
        // Painted while an outer frame holds the state: validate the update
        // region only and repaint the strip from the message loop.
        let mut paint = PAINTSTRUCT::default();
        unsafe {
            let _ = BeginPaint(hwnd, &mut paint);
            let _ = EndPaint(hwnd, &paint);
        }
        input::defer_repaint();
        return;
    }
    let (tabs, selected, focus_visible, colors, dpi, theme, font) = {
        let state = state.borrow();
        let app = state.portable.borrow();
        (
            app.tabs()
                .tabs()
                .iter()
                .map(|tab| (tab.id(), tab.title().to_owned()))
                .collect::<Vec<_>>(),
            app.active_tab(),
            app.focus_is_visible(),
            palette(state.theme),
            state.dpi,
            state.theme,
            state.chrome.text_fonts().map(|fonts| fonts.0),
        )
    };
    let count = tabs.len();
    // The order a queued click is pinned against (`input`).
    input::snapshot_tabs(&tabs.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    let hot = TAB_HOT.with(Cell::get);
    let px = |value: u32| i32::try_from(chrome::dip(value, dpi)).unwrap_or(0);
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
        let previous_font = font.map(|font| unsafe { SelectObject(dc, HGDIOBJ(font.0)) });
        // The strip: the command bar's surface, a one-pixel line below.
        fill(dc, &client, native_color(colors.surface));
        fill(
            dc,
            &RECT {
                top: client.bottom - 1,
                ..client
            },
            native_color(colors.border),
        );
        let width = client.right - client.left;
        let extent = tabs_extent(
            width,
            count,
            i32::try_from(Dip::TAB_MAX_WIDTH.to_pixels(dpi)).unwrap_or(i32::MAX),
        );
        let minimum = i32::try_from(Dip::INTERACTION_TARGET.to_pixels(dpi)).unwrap_or(44);
        let hot_hit =
            hot.and_then(|x| accessibility::hit_test_tab(extent, client.bottom, minimum, count, x));
        for (index, (tab_id, title)) in tabs.iter().enumerate() {
            let Some((left, right)) = tab_slot(extent, count, index) else {
                continue;
            };
            let slot = RECT {
                left,
                top: 0,
                right,
                bottom: client.bottom,
            };
            let active = selected == Some(*tab_id);
            // The visible tab: 2 DIPs apart, top corners rounded by 6 DIPs
            // (the round rectangle runs below the strip, so only the top
            // corners show).
            let tab = RECT {
                left: slot.left + px(2),
                top: slot.top + px(4),
                right: slot.right - px(2),
                bottom: slot.bottom,
            };
            let body_hot =
                matches!(hot_hit, Some(TabHit::Body(i) | TabHit::Close(i)) if i == index);
            let face = if active {
                Some(colors.elevated)
            } else if body_hot && theme != EffectiveTheme::HighContrast {
                Some(colors.hover)
            } else {
                None
            };
            if let Some(face) = face {
                style::fill_round(
                    dc,
                    &RECT {
                        bottom: tab.bottom + px(6),
                        ..tab
                    },
                    native_color(face),
                    px(6),
                );
                // The strip line stays under inactive tabs only.
                fill(
                    dc,
                    &RECT {
                        top: client.bottom - 1,
                        ..tab
                    },
                    native_color(face),
                );
            }
            if active {
                // The selected-tab indicator: a 3-DIP accent line.
                fill(
                    dc,
                    &RECT {
                        left: tab.left + px(8),
                        right: tab.right - px(8),
                        top: tab.bottom - px(3),
                        bottom: tab.bottom,
                    },
                    native_color(style::indicator(theme, colors)),
                );
            }
            let close_hot = matches!(hot_hit, Some(TabHit::Close(i)) if i == index);
            input::paint_tab_content(
                dc,
                &slot,
                title,
                native_color(input::tab_text_color(theme, colors, active)),
                (close_hot && theme != EffectiveTheme::HighContrast)
                    .then(|| native_color(colors.hover)),
                dpi,
            );
            if active && focus_visible {
                let _ = unsafe { DrawFocusRect(dc, &style::inset(&tab, px(2), px(2))) };
            }
        }
        if let Some(previous) = previous_font {
            unsafe { SelectObject(dc, previous) };
        }
    }
    let _ = unsafe { EndPaint(hwnd, &paint) };
}

thread_local! {
    /// The pointer's x in the tab strip while it is over it (hover).
    static TAB_HOT: Cell<Option<i32>> = const { Cell::new(None) };
}

/// Tracks the pointer over the tab strip for the hover look. Reads no
/// shell state; only repaints the strip.
fn track_tab_hover(hwnd: HWND, x: Option<i32>) {
    let previous = TAB_HOT.with(|hot| hot.replace(x));
    if previous.is_none() && x.is_some() {
        let mut track = windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT>(
            ) as u32,
            dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(&mut track) };
    }
    if previous != x {
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

fn fill(dc: windows::Win32::Graphics::Gdi::HDC, rect: &RECT, color: COLORREF) {
    let brush = unsafe { CreateSolidBrush(color) };
    unsafe {
        FillRect(dc, rect, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

fn native_color(spec: ColorSpec) -> COLORREF {
    match spec {
        ColorSpec::Rgb(rgb) => {
            COLORREF(((rgb & 0xff) << 16) | (rgb & 0x00ff00) | ((rgb >> 16) & 0xff))
        }
        ColorSpec::System(role) => COLORREF(unsafe {
            GetSysColor(match role {
                SystemColorRole::Window => COLOR_WINDOW,
                SystemColorRole::WindowText => COLOR_WINDOWTEXT,
                SystemColorRole::Highlight => COLOR_HIGHLIGHT,
                SystemColorRole::HighlightText => COLOR_HIGHLIGHTTEXT,
                SystemColorRole::ButtonFace => COLOR_BTNFACE,
                SystemColorRole::ButtonText => COLOR_BTNTEXT,
                SystemColorRole::GrayText => COLOR_GRAYTEXT,
            })
        }),
    }
}

fn route_native_key(hwnd: HWND, key: u16) -> bool {
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    route_native_chord(hwnd, key, ctrl, shift)
}

/// Runs a native shortcut with the modifiers as pressed. While an outer
/// frame holds the state (a key dispatched by a nested modal loop) a key
/// that maps to a shell command is queued and runs from the message loop.
fn route_native_chord(hwnd: HWND, key: u16, ctrl: bool, shift: bool) -> bool {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return false;
    };
    if !ctrl && !shift && matches!(key, 0x25 | 0x27) {
        // Left and Right belong to the tab strip only while it has focus.
        // The strip handle is a copy outside the state, so this also holds
        // while the state is mutably borrowed.
        let tab_strip = input::tab_strip_window();
        if tab_strip.is_none() || Some(unsafe { GetFocus() }) != tab_strip {
            return false;
        }
    }
    let Some(command) = route_key(KeyChord::new(key, ctrl, shift, false)) else {
        return false;
    };
    if input::has_pending() || !shell_free(&state) {
        input::defer_native_key(key, ctrl, shift);
        return true;
    }
    execute_command(hwnd, command)
}

fn execute_command(hwnd: HWND, command: ShellCommand) -> bool {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return false;
    };
    let portable = Rc::clone(&state.borrow().portable);
    portable.borrow_mut().note_keyboard_input();
    if command == ShellCommand::CloseTab {
        // The same close as the close button and UIA Invoke.
        let active = portable.borrow().active_tab();
        return active.is_some_and(|active| input::close_tab(hwnd, active));
    }
    let previous = portable.borrow().active_tab();
    let applied = portable.borrow_mut().apply_command(command);
    if !applied {
        return false;
    }
    // F6 skips the hidden find bar: focus on its hidden box would let
    // Enter open the bar without Ctrl+F or the Find button.
    if command == ShellCommand::CycleFocus {
        let hidden = !portable.borrow().find_bar_visible();
        if hidden && portable.borrow().focused_surface() == FocusSurface::FindBar {
            let _ = portable.borrow_mut().apply_command(command);
        }
    }
    if command == ShellCommand::CycleFocus {
        let (controls, focus) = {
            let state = state.borrow();
            (state.controls, portable.borrow().focused_surface())
        };
        if focus == FocusSurface::Document {
            // Logical Document focus lives inside the active WebView.
            let moved = focus_document(&state);
            if !moved && let Some(controls) = controls {
                let _ = unsafe { SetFocus(Some(controls.tabs)) };
            }
        } else if let Some(controls) = controls {
            let target = focus_target(controls, focus);
            let _ = unsafe { SetFocus(Some(target)) };
        }
        refresh_tabs(&state, None);
        return true;
    }
    // Bind the active tab first: a borrow held in an `if let` scrutinee would
    // span the reentrant WebView COM call below.
    let active = portable.borrow().active_tab();
    // Commands that keep the active tab (sidebar, find bar) need no switch.
    let switched = active != previous;
    if switched
        && let Some(active) = active
        && !select_webview(&state, active.get())
    {
        // Keep portable state and the visible controller in agreement.
        if let Some(previous) = previous {
            let _ = portable.borrow_mut().select_tab(previous);
        }
        refresh_tabs(&state, None);
        return false;
    }
    refresh_tabs(&state, active.filter(|id| Some(*id) != previous));
    true
}

/// Moves keyboard focus into the active document's WebView controller.
fn focus_document(state: &Rc<RefCell<AppState>>) -> bool {
    let active = state.borrow().webview_active;
    let Some(session) = take_session(state) else {
        return false;
    };
    let moved = active
        .and_then(|tab| session.host(tab))
        .is_some_and(|host| host.move_focus().is_ok());
    restore_session(state, session);
    moved
}

fn focus_target(c: ShellControls, focus: FocusSurface) -> HWND {
    match focus {
        FocusSurface::CommandBar => c.command,
        FocusSurface::TabStrip => c.tabs,
        FocusSurface::Repository => c.sidebar,
        FocusSurface::Document => c.tabs,
        FocusSurface::FindBar => c.find,
        FocusSurface::Status => c.status,
    }
}

fn select_tab_at(hwnd: HWND, x: i32) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    let (portable, width, dpi) = {
        let state = state.borrow();
        (
            Rc::clone(&state.portable),
            state
                .accessibility
                .as_ref()
                .map_or(1, NativeAccessibility::tab_strip_width)
                .max(1),
            state.dpi,
        )
    };
    portable.borrow_mut().note_pointer_input();
    let count = portable.borrow().tabs().tabs().len();
    if count == 0 {
        return;
    }
    let width = tabs_extent(
        i32::try_from(width).unwrap_or(i32::MAX),
        count,
        i32::try_from(Dip::TAB_MAX_WIDTH.to_pixels(dpi)).unwrap_or(i32::MAX),
    );
    let index = (0..count)
        .find(|index| tab_slot(width, count, *index).is_some_and(|(_, right)| x < right))
        .unwrap_or(count - 1);
    let id = portable.borrow().tabs().tabs()[index].id();
    let _ = select_tab_id(hwnd, id.get(), true);
}

/// Selects a tab from pointer, keyboard, or UIA input. Returns true only when
/// portable state, the active WebView controller, and the tab strip agree on
/// `raw_id`. The WebView switches first so a failure leaves nothing changed.
fn select_tab_id(hwnd: HWND, raw_id: u64, pointer_input: bool) -> bool {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return false;
    };
    let portable = Rc::clone(&state.borrow().portable);
    let id = portable
        .borrow()
        .tabs()
        .tabs()
        .iter()
        .find(|tab| tab.id().get() == raw_id)
        .map(|tab| tab.id());
    let Some(id) = id else { return false };
    if pointer_input {
        portable.borrow_mut().note_pointer_input();
    }
    let already_active = portable.borrow().active_tab() == Some(id);
    if already_active {
        return true;
    }
    if !select_webview(&state, id.get()) {
        return false;
    }
    let selected = portable.borrow_mut().select_tab(id);
    if !selected {
        // The tab closed during the WebView call; restore the controller for
        // whichever tab portable state still considers active.
        let active = portable.borrow().active_tab();
        if let Some(active) = active {
            let _ = select_webview(&state, active.get());
        }
        refresh_tabs(&state, None);
        return false;
    }
    refresh_tabs(&state, Some(id));
    true
}

/// Repaints the tab strip synchronously and, when a new tab became active,
/// raises the UIA selection event after the paint so clients observe
/// agreement.
fn refresh_tabs(state: &Rc<RefCell<AppState>>, newly_selected: Option<TabId>) {
    let Some((a11y, controls)) =
        crate::reentry::read(state, |state| (state.accessibility.clone(), state.controls))
    else {
        return;
    };
    let order = crate::reentry::read(state, |state| {
        crate::reentry::read(&state.portable, |app| {
            app.tabs()
                .tabs()
                .iter()
                .map(|tab| tab.id())
                .collect::<Vec<_>>()
        })
    })
    .flatten();
    if let Some(order) = order {
        input::snapshot_tabs(&order);
    }
    if let Some(c) = controls {
        unsafe {
            let _ = InvalidateRect(Some(c.tabs), None, true);
            let _ = UpdateWindow(c.tabs);
        }
    }
    if let (Some(a11y), Some(id)) = (a11y.as_ref(), newly_selected) {
        a11y.notify_selected(id);
    }
    if newly_selected.is_some() {
        // A notice concerned the previous view.
        let _ = crate::reentry::write(state, |state| state.notice = None);
        // .NET `DocumentTabs_SelectionChanged` → `RefreshSearchForActiveTab`.
        input::refresh_find(state);
    }
    refresh_status(state);
    update_window_title(state);
    // chrome: outline and history buttons follow the active tab.
    chrome::tabs_changed(state);
}

fn poll_webview(hwnd: HWND) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    let Some(mut session) = take_session(&state) else {
        return;
    };
    let error = session.poll().err();
    // An environment failure disables every document; other session errors
    // are reported in the status bar. Tab-scoped failures (controller
    // startup or setup, failed first navigation) come separately.
    let environment_failure = error
        .as_ref()
        .is_some_and(marknexia_webview::session::SessionError::is_environment_failure);
    let error = error.map(|error| format!("WebView2 poll: {error:?}"));
    let tab_failures = session.drain_tab_failures();
    let (active, document_mode, rendered_tabs) = {
        let state = state.borrow();
        (
            state.webview_active,
            state.startup.document_mode(),
            state.rendered_tabs.iter().copied().collect::<Vec<_>>(),
        )
    };
    let (controller_ready, active_loaded) = active
        .and_then(|tab| session.host(tab))
        .map_or((false, false), |host| (true, host.document_loaded()));
    // With document paths, first render means a rendered Markdown document
    // finished navigating, whichever tab it is in; otherwise the active
    // placeholder.
    let loaded_tabs: Vec<u64> = if document_mode {
        rendered_tabs
            .iter()
            .copied()
            .filter(|tab| {
                session
                    .host(*tab)
                    .is_some_and(|host| host.document_loaded())
            })
            .collect()
    } else {
        active.filter(|_| active_loaded).into_iter().collect()
    };
    let accelerators = session.drain_accelerators();
    restore_session(&state, session);
    {
        let mut state = state.borrow_mut();
        for (tab_id, failure) in &tab_failures {
            state.tab_errors.insert(
                *tab_id,
                format!("The document view could not start ({failure:?})."),
            );
            let signal = state.startup.failed(*tab_id);
            state.apply_startup(signal);
        }
        for tab_id in &loaded_tabs {
            // A document that loads clears any earlier failure for its tab
            // (a recreated environment retried it).
            state.tab_errors.remove(tab_id);
            let signal = state.startup.rendered(*tab_id);
            state.apply_startup(signal);
        }
        if controller_ready && let Some(readiness) = state.readiness.as_mut() {
            readiness.controller_ready();
        }
    }
    // Page messages (find results) and newly loaded documents.
    input::after_poll(hwnd);
    // Shell chords pressed inside the document run here, outside COM.
    for chord in accelerators {
        if input::route_accelerator(hwnd, chord.virtual_key, chord.ctrl, chord.shift, chord.alt) {
            continue;
        }
        if let Some(command) = route_key(KeyChord::new(
            chord.virtual_key,
            chord.ctrl,
            chord.shift,
            chord.alt,
        )) {
            let _ = execute_command(hwnd, command);
        } else {
            // chrome: the .NET command accelerators.
            chrome::route_accelerator(hwnd, chord);
        }
    }
    // chrome: restore reloaded documents' locations once loaded.
    chrome::after_poll(&state);
    if let Some(error) = error {
        {
            let mut state = state.borrow_mut();
            state.shell_error = Some(error);
            if environment_failure {
                let signal = state.startup.webview_unavailable();
                state.apply_startup(signal);
            }
        }
        refresh_status(&state);
    } else if !tab_failures.is_empty() {
        refresh_status(&state);
    }
}
/// Named manual-reset events that let an external measurement harness
/// observe startup milestones of this process without UI scraping:
/// `Local\Marknexia.WebViewReady.<pid>` when the active tab's controller
/// exists, `Local\Marknexia.FirstRender.<pid>` when its document (with
/// document paths: the first rendered Markdown document) has completed
/// navigation, and `Local\Marknexia.StartupFailed.<pid>` when every startup
/// document failed, so a harness fails fast. Signalling is best-effort and
/// never affects the UI.
struct Readiness {
    webview_ready: Option<HANDLE>,
    first_render: Option<HANDLE>,
    startup_failed: Option<HANDLE>,
    webview_signaled: bool,
    render_signaled: bool,
    failure_signaled: bool,
    /// update: the nonce of a `--post-update-health` start, until the
    /// acknowledgement is written.
    health: Option<String>,
    shell_visible: bool,
}

impl Readiness {
    fn create(health: Option<String>) -> Self {
        let pid = std::process::id();
        Self {
            webview_ready: named_event(&format!(r"Local\Marknexia.WebViewReady.{pid}")),
            first_render: named_event(&format!(r"Local\Marknexia.FirstRender.{pid}")),
            startup_failed: named_event(&format!(r"Local\Marknexia.StartupFailed.{pid}")),
            webview_signaled: false,
            render_signaled: false,
            failure_signaled: false,
            health,
            shell_visible: false,
        }
    }

    fn shell_shown(&mut self, visible: bool) {
        self.shell_visible = visible;
        self.acknowledge_health();
    }

    fn controller_ready(&mut self) {
        signal_once(&mut self.webview_ready, &mut self.webview_signaled);
        self.acknowledge_health();
    }

    /// Exactly one of these is ever raised: `StartupTracker` decides once.
    fn signal(&mut self, signal: StartupSignal) {
        match signal {
            StartupSignal::FirstRender => {
                signal_once(&mut self.first_render, &mut self.render_signaled);
            }
            StartupSignal::StartupFailed => {
                signal_once(&mut self.startup_failed, &mut self.failure_signaled);
            }
        }
        self.acknowledge_health();
    }

    /// update: in health mode, once the shell is visible, WebView2 is ready
    /// and the offline welcome document rendered, or once startup failed,
    /// writes the nonce-bound acknowledgement (a failed one lets the old
    /// version roll back at once instead of waiting for the deadline).
    fn acknowledge_health(&mut self) {
        let probes = marknexia_update::activation::Probes {
            shell_visible: self.shell_visible,
            web_view_ready: self.webview_signaled,
            offline_render: self.render_signaled,
        };
        if !(probes.all_passed() || self.failure_signaled) {
            return;
        }
        let Some(nonce) = self.health.take() else {
            return;
        };
        if let Ok(executable) = std::env::current_exe() {
            let _ = marknexia_update::activation::write_health_ack(
                &executable,
                &nonce,
                std::process::id(),
                probes,
            );
        }
    }
}

impl Drop for Readiness {
    fn drop(&mut self) {
        for handle in [
            self.webview_ready.take(),
            self.first_render.take(),
            self.startup_failed.take(),
        ]
        .into_iter()
        .flatten()
        {
            // SAFETY: each handle came from CreateEventW and is closed once.
            let _ = unsafe { CloseHandle(handle) };
        }
    }
}

fn named_event(name: &str) -> Option<HANDLE> {
    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `name` is NUL-terminated and outlives the call; the returned
    // handle is owned by `Readiness` and closed exactly once in Drop.
    unsafe { CreateEventW(None, true, false, PCWSTR::from_raw(name.as_ptr())) }.ok()
}

/// Sets the event once. The handle stays open until the process tears down
/// so a harness that opens the name late still observes the signal.
fn signal_once(slot: &mut Option<HANDLE>, signaled: &mut bool) {
    if let (Some(handle), false) = (slot.as_ref(), *signaled) {
        *signaled = true;
        // SAFETY: the handle is a live event owned by Readiness.
        let _ = unsafe { SetEvent(*handle) };
    }
}

fn reflow(hwnd: HWND, dpi: u32) {
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        return;
    }
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    // WM_SIZE and WM_DPICHANGED can be sent while an outer frame holds the
    // state: lay out from the message loop instead.
    if !shell_free(&state) {
        input::defer_reflow(dpi);
        return;
    }
    let (portable, controls) = {
        let state = state.borrow();
        (Rc::clone(&state.portable), state.controls)
    };
    let actual_dpi = dpi.max(96);
    state.borrow_mut().dpi = actual_dpi;
    // chrome: the .NET compact layout below 720 DIPs (pane closed).
    chrome::update_compact(&state, client.right.max(0) as u32, actual_dpi);
    // chrome: persisted sidebar width and the diagnostics bar.
    let (sidebar_width, diagnostics_open) = state.borrow().chrome.layout_inputs();
    let app = portable.borrow();
    let layout = ShellLayout::compute(
        ShellLayoutRequest::new(
            client.right.max(0) as u32,
            client.bottom.max(0) as u32,
            actual_dpi,
        )
        .with_sidebar_visible(app.sidebar_visible())
        .with_find_bar_visible(app.find_bar_visible())
        .with_sidebar_width(sidebar_width)
        .with_diagnostics_visible(diagnostics_open),
    );
    drop(app);
    BANDS.with(|bands| {
        bands.set([
            layout.command_bar,
            layout.sidebar,
            layout.find_bar,
            layout.status_bar,
        ]);
    });
    // The shell window's own bands (command bar, pane, status) move too.
    let _ = unsafe { InvalidateRect(Some(hwnd), None, true) };
    if let Some(c) = controls {
        for (window, rect) in [
            (c.command, layout.command_bar),
            (c.tabs, layout.tab_strip),
            (c.sidebar, layout.sidebar),
            (c.find, layout.find_bar),
            (c.status, status_text_rect(layout.status_bar, actual_dpi)),
        ] {
            move_child(window, rect);
        }
        let _ = unsafe {
            ShowWindow(
                c.sidebar,
                if layout.sidebar.width == 0 {
                    SW_HIDE
                } else {
                    SW_SHOW
                },
            )
        };
        let _ = unsafe {
            ShowWindow(
                c.find,
                if layout.find_bar.height == 0 {
                    SW_HIDE
                } else {
                    SW_SHOW
                },
            )
        };
    }
    input::layout_find_bar(&state, layout.find_bar, actual_dpi);
    if let Some(mut session) = take_session(&state) {
        let b = layout.webview;
        let _ = session.set_viewport(ViewportBounds {
            left: b.x as i32,
            top: b.y as i32,
            right: b.right() as i32,
            bottom: b.bottom() as i32,
        });
        restore_session(&state, session);
    }
    let a11y = state.borrow().accessibility.clone();
    if let Some(a11y) = a11y.as_ref() {
        a11y.update_tab_strip_bounds(layout.tab_strip);
    }
    // chrome: toolbar, sidebar and diagnostics bar placement.
    chrome::layout_children(hwnd, &layout);
    // runtime: the missing-runtime state fills the document area.
    runtime::layout(&state, layout.webview, actual_dpi);
}

/// The status text inside the status bar: 12 DIPs in from each side, below
/// the bar's one-pixel top line (painted by the shell window).
fn status_text_rect(bar: PixelRect, dpi: u32) -> PixelRect {
    let inset = chrome::dip(12, dpi).min(bar.width / 2);
    let line = u32::from(bar.height > 0);
    PixelRect {
        x: bar.x.saturating_add(inset),
        y: bar.y.saturating_add(line),
        width: bar.width.saturating_sub(inset.saturating_mul(2)),
        height: bar.height.saturating_sub(line),
    }
}

fn move_child(hwnd: HWND, b: PixelRect) {
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            b.x as i32,
            b.y as i32,
            b.width as i32,
            b.height as i32,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    };
}

fn update_theme(hwnd: HWND) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    // Setting broadcasts are sent synchronously, also into nested pumps.
    if !shell_free(&state) {
        input::defer_theme();
        return;
    }
    let (preference, controls) = {
        let state = state.borrow();
        (state.portable.borrow().theme(), state.controls)
    };
    let theme = resolve_theme(preference, system_prefers_dark(), high_contrast());
    let brushes = ThemeBrushes::for_theme(theme);
    let previous = {
        let mut state = state.borrow_mut();
        state.theme = theme;
        std::mem::replace(&mut state.brushes, brushes)
    };
    // Controls may still hold the old brush until they repaint below; it is
    // deleted only after the new one is installed.
    drop(previous);
    apply_window_theme(hwnd, theme);
    if let Some(c) = controls {
        apply_control_themes(c, theme);
    }
    if let Some(mut session) = take_session(&state) {
        let _ = session.set_background(webview_color(palette(theme).background));
        restore_session(&state, session);
    }
    if let Some(c) = controls {
        for window in [c.command, c.tabs, c.sidebar, c.find, c.status] {
            let _ = unsafe { InvalidateRect(Some(window), None, true) };
        }
    }
    // chrome: tree colors and control visual styles.
    chrome::theme_changed(&state);
}

/// GDI brushes for dark-theme standard controls, deleted on drop.
struct ThemeBrushes {
    control: HBRUSH,
    background: HBRUSH,
    text: COLORREF,
    control_color: COLORREF,
}

impl ThemeBrushes {
    fn for_theme(theme: EffectiveTheme) -> Option<Self> {
        if theme != EffectiveTheme::Dark {
            return None;
        }
        let colors = palette(theme);
        let control_color = native_color(colors.surface);
        let background_color = native_color(colors.background);
        let control = unsafe { CreateSolidBrush(control_color) };
        let background = unsafe { CreateSolidBrush(background_color) };
        if control.is_invalid() || background.is_invalid() {
            for brush in [control, background] {
                if !brush.is_invalid() {
                    let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
                }
            }
            return None;
        }
        Some(Self {
            control,
            background,
            text: native_color(colors.text),
            control_color,
        })
    }
}

impl Drop for ThemeBrushes {
    fn drop(&mut self) {
        for brush in [self.control, self.background] {
            let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
        }
    }
}

/// Visual styles for standard controls: dark variants in the dark theme,
/// the default theme otherwise (high contrast uses system colors).
fn apply_control_themes(c: ShellControls, theme: EffectiveTheme) {
    let dark = theme == EffectiveTheme::Dark;
    for (window, dark_class) in [
        (c.command, w!("DarkMode_Explorer")),
        (c.sidebar, w!("DarkMode_Explorer")),
        (c.find, w!("DarkMode_CFD")),
    ] {
        let _ = unsafe {
            if dark {
                SetWindowTheme(window, dark_class, PCWSTR::null())
            } else {
                SetWindowTheme(window, PCWSTR::null(), PCWSTR::null())
            }
        };
    }
}

/// Paints the owner-drawn command button from the active palette, so light,
/// dark, and high-contrast (system colors) themes all apply.
fn draw_command_button(hwnd: HWND, item: &DRAWITEMSTRUCT) -> bool {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return false;
    };
    let (command, theme) = {
        let Ok(state) = state.try_borrow() else {
            return false;
        };
        (state.controls.map(|c| c.command), state.theme)
    };
    if command != Some(item.hwndItem) {
        return false;
    }
    let colors = palette(theme);
    let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
    let (face, text) = if pressed {
        (colors.accent_pressed, colors.on_accent)
    } else if disabled {
        (colors.surface, colors.text_disabled)
    } else {
        (colors.elevated, colors.text)
    };
    let dc = item.hDC;
    let rect = item.rcItem;
    fill(dc, &rect, native_color(face));
    let border = unsafe { CreateSolidBrush(native_color(colors.border)) };
    if !border.is_invalid() {
        unsafe {
            FrameRect(dc, &rect, border);
            let _ = DeleteObject(HGDIOBJ(border.0));
        }
    }
    let mut label = [0_u16; 128];
    let length = unsafe { GetWindowTextW(item.hwndItem, &mut label) };
    let length = usize::try_from(length).unwrap_or(0).min(label.len());
    let mut text_rect = rect;
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, native_color(text));
        style::draw_text(
            dc,
            &mut label[..length],
            &mut text_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
    }
    if item.itemState.0 & ODS_FOCUS.0 != 0 {
        let mut focus = rect;
        focus.left += 3;
        focus.top += 3;
        focus.right -= 3;
        focus.bottom -= 3;
        let _ = unsafe { DrawFocusRect(dc, &focus) };
    }
    true
}

fn themed_control_brush(hwnd: HWND, dc: HDC) -> Option<HBRUSH> {
    let state = unsafe { app_state_handle(hwnd) }?;
    let state = state.try_borrow().ok()?;
    let brushes = state.brushes.as_ref()?;
    unsafe {
        SetTextColor(dc, brushes.text);
        SetBkColor(dc, brushes.control_color);
    }
    Some(brushes.control)
}

thread_local! {
    /// The last layout's command bar, sidebar, find bar and status bar, for
    /// painting the shell window's own background (`WM_ERASEBKGND`).
    static BANDS: Cell<[PixelRect; 4]> = const { Cell::new([PixelRect { x: 0, y: 0, width: 0, height: 0 }; 4]) };
}

fn to_rect(rect: PixelRect) -> RECT {
    RECT {
        left: i32::try_from(rect.x).unwrap_or(i32::MAX),
        top: i32::try_from(rect.y).unwrap_or(i32::MAX),
        right: i32::try_from(rect.right()).unwrap_or(i32::MAX),
        bottom: i32::try_from(rect.bottom()).unwrap_or(i32::MAX),
    }
}

/// Paints the shell window's background in every theme: the page color,
/// the command bar and the find bar on the surface color with a one-pixel
/// line below (.NET command grid `BorderThickness="0,0,0,1"`), the sidebar
/// on its pane color, and the status bar on the surface with a line above.
/// High contrast uses system colors only.
fn erase_themed_background(hwnd: HWND, dc: HDC) -> bool {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return false;
    };
    let Ok(theme) = state.try_borrow().map(|state| state.theme) else {
        return false;
    };
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        return false;
    }
    let colors = palette(theme);
    let high_contrast = theme == EffectiveTheme::HighContrast;
    let (page, bar, line) = if high_contrast {
        (
            ColorSpec::System(SystemColorRole::Window),
            ColorSpec::System(SystemColorRole::Window),
            ColorSpec::System(SystemColorRole::WindowText),
        )
    } else {
        (colors.background, colors.surface, colors.border)
    };
    let sidebar = style::sidebar_colors(theme, colors);
    let [command, side, find, status] = BANDS.with(Cell::get);
    fill(dc, &client, native_color(page));
    for band in [command, find] {
        if band.height > 0 {
            let rect = to_rect(band);
            fill(dc, &rect, native_color(bar));
            fill(
                dc,
                &RECT {
                    top: rect.bottom - 1,
                    ..rect
                },
                native_color(line),
            );
        }
    }
    if side.width > 0 {
        fill(dc, &to_rect(side), native_color(sidebar.background));
    }
    if status.height > 0 {
        let rect = to_rect(status);
        fill(dc, &rect, native_color(bar));
        fill(
            dc,
            &RECT {
                bottom: rect.top + 1,
                ..rect
            },
            native_color(line),
        );
    }
    true
}

fn system_prefers_dark() -> bool {
    let mut light_theme = 1_u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut light_theme as *mut u32).cast::<c_void>()),
            Some(&mut size),
        )
    };
    status.is_ok() && size == std::mem::size_of::<u32>() as u32 && light_theme == 0
}

/// The title bar follows the theme (Windows 11; Windows 10 ignores the
/// color attributes, and every failure is ignored): immersive dark mode in
/// the dark theme, and the caption, caption text and border colors of the
/// palette (the caption on the command bar's surface color, so the two
/// read as one band). High contrast restores the system colors
/// (`DWMWA_COLOR_DEFAULT`). The system caption buttons stay.
fn apply_window_theme(hwnd: HWND, theme: EffectiveTheme) {
    use windows::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWINDOWATTRIBUTE,
    };
    /// `DWMWA_COLOR_DEFAULT`: the system's own color.
    const COLOR_DEFAULT: u32 = 0xFFFF_FFFF;
    let set = |attribute: DWMWINDOWATTRIBUTE, value: u32| {
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute,
                (&value as *const u32).cast::<c_void>(),
                std::mem::size_of_val(&value) as u32,
            )
        };
    };
    set(
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        u32::from(matches!(theme, EffectiveTheme::Dark)),
    );
    let colors = palette(theme);
    let (caption, text, border) = if theme == EffectiveTheme::HighContrast {
        (COLOR_DEFAULT, COLOR_DEFAULT, COLOR_DEFAULT)
    } else {
        (
            native_color(colors.surface).0,
            native_color(colors.text).0,
            native_color(colors.border).0,
        )
    };
    set(DWMWA_CAPTION_COLOR, caption);
    set(DWMWA_TEXT_COLOR, text);
    set(DWMWA_BORDER_COLOR, border);
}

/// The window title: "Marknexia — <document>" for the active tab, as the
/// .NET store edition shows it, else "Marknexia".
pub(crate) fn window_title(document: Option<&str>) -> String {
    match document.filter(|title| !title.is_empty()) {
        Some(title) => format!("Marknexia \u{2014} {title}"),
        None => "Marknexia".to_owned(),
    }
}

fn update_window_title(state: &Rc<RefCell<AppState>>) {
    let Some((window, title)) = crate::reentry::read(state, |state| {
        let title = crate::reentry::read(&state.portable, |app| {
            let active = app.active_tab()?;
            // Placeholder tabs (no document) keep the plain product name.
            state.chrome.has_document(active.get()).then_some(())?;
            app.tabs()
                .tabs()
                .iter()
                .find(|tab| tab.id() == active)
                .map(|tab| tab.title().to_owned())
        })
        .flatten();
        (state.chrome.window(), title)
    }) else {
        return;
    };
    let Some(window) = window else {
        return;
    };
    let text: Vec<u16> = window_title(title.as_deref())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: NUL-terminated buffer that outlives the call; no borrow held.
    let _ = unsafe { SetWindowTextW(window, PCWSTR::from_raw(text.as_ptr())) };
}

fn webview_color(spec: ColorSpec) -> HostColor {
    let color = native_color(spec).0;
    HostColor {
        alpha: 255,
        red: (color & 0xff) as u8,
        green: ((color >> 8) & 0xff) as u8,
        blue: ((color >> 16) & 0xff) as u8,
    }
}

fn take_session(state: &Rc<RefCell<AppState>>) -> Option<WebViewSession> {
    let mut state = state.borrow_mut();
    let session = state.webview.take();
    if session.is_some() {
        state.webview_checked_out = true;
    }
    session
}
fn restore_session(state: &Rc<RefCell<AppState>>, session: WebViewSession) {
    let mut state = state.borrow_mut();
    state.webview_checked_out = false;
    if !state.destroyed {
        state.webview = Some(session);
    }
}
/// Makes `tab_id` the visible controller. Returns false when the session is
/// checked out by an outer call or rejects the selection. Without a WebView
/// session (runtime unavailable) there is no controller to disagree with.
fn select_webview(state: &Rc<RefCell<AppState>>, tab_id: u64) -> bool {
    if state.borrow().webview_checked_out {
        return false;
    }
    let Some(mut session) = take_session(state) else {
        return true;
    };
    // A tab whose render is pending has no controller: show none.
    let has_document = session.has_document(tab_id);
    let result = if has_document {
        session.select_tab(tab_id)
    } else {
        session.clear_selection()
    };
    let error = result
        .err()
        .map(|error| format!("WebView2 select: {error:?}"));
    restore_session(state, session);
    match error {
        Some(error) => {
            state.borrow_mut().notice = Some(error);
            false
        }
        None => {
            state.borrow_mut().webview_active = has_document.then_some(tab_id);
            true
        }
    }
}

/// UIA `Select` handler; see the acknowledgement contract in `initialize`.
fn select_from_automation(hwnd: HWND, id: TabId) -> bool {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return false;
    };
    // UIA can call in while an outer frame holds the state (a nested pump).
    // A shared outer borrow still lets the portable selection change now,
    // so IsSelected agrees on return; only the repaint waits for the loop.
    // When even that is impossible, UIA clients (which do not retry) get
    // S_OK and the selection is applied from the message loop, like the
    // deferred WebView switch; it raises ElementSelected then. A tab closed
    // meanwhile is dropped.
    let Some(portable) = crate::reentry::read(&state, |state| Rc::clone(&state.portable))
        .filter(|portable| crate::reentry::is_free(portable))
    else {
        input::defer_automation_select(id);
        return true;
    };
    let already_active = portable.borrow().active_tab() == Some(id);
    if already_active {
        return true;
    }
    let selected = portable.borrow_mut().select_tab(id);
    if !selected {
        return false;
    }
    if shell_free(&state) {
        refresh_tabs(&state, Some(id));
    } else {
        // The strip cannot repaint now: the repaint and ElementSelected
        // follow together from the loop, so clients observe agreement.
        input::defer_selected(id);
    }
    let posted = unsafe {
        PostMessageW(
            Some(hwnd),
            WM_APPLY_WEBVIEW_SELECTION,
            WPARAM(id.get() as usize),
            LPARAM(0),
        )
    };
    if posted.is_err() {
        revert_to_webview_tab(&state);
        return false;
    }
    true
}

/// Runs outside any UIA call, so WebView2 COM is allowed here.
fn apply_automation_selection(hwnd: HWND, raw_id: u64) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    if !shell_free(&state) {
        // Dispatched by a nested pump: run it from the message loop.
        input::defer_selection(raw_id);
        return;
    }
    let portable = Rc::clone(&state.borrow().portable);
    let active = portable.borrow().active_tab();
    // A later selection superseded this one; it posted its own switch.
    if active.map(TabId::get) != Some(raw_id) {
        return;
    }
    if !select_webview(&state, raw_id) {
        revert_to_webview_tab(&state);
    }
}

/// Restores portable selection to the tab the WebView actually shows.
fn revert_to_webview_tab(state: &Rc<RefCell<AppState>>) {
    let (portable, shown) = {
        let state = state.borrow();
        (Rc::clone(&state.portable), state.webview_active)
    };
    let target = shown.and_then(|raw| {
        portable
            .borrow()
            .tabs()
            .tabs()
            .iter()
            .find(|tab| tab.id().get() == raw)
            .map(|tab| tab.id())
    });
    if let Some(target) = target {
        let _ = portable.borrow_mut().select_tab(target);
        refresh_tabs(state, Some(target));
    }
}

fn high_contrast() -> bool {
    let mut value = windows::Win32::UI::Accessibility::HIGHCONTRASTW {
        cbSize: std::mem::size_of::<windows::Win32::UI::Accessibility::HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            windows::Win32::UI::WindowsAndMessaging::SPI_GETHIGHCONTRAST,
            value.cbSize,
            Some((&mut value as *mut windows::Win32::UI::Accessibility::HIGHCONTRASTW).cast()),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
            && value.dwFlags.0 & windows::Win32::UI::Accessibility::HCF_HIGHCONTRASTON.0 != 0
    }
}

fn webview_folder() -> PathBuf {
    // Measurement harnesses may isolate the WebView2 profile (cold starts);
    // only an absolute path is honored.
    if let Some(folder) = std::env::var_os("MARKNEXIA_WEBVIEW2_USER_DATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return folder;
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Marknexia")
        .join("WebView2")
}

fn enable_dpi() -> Result<(), Error> {
    match unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        Ok(()) => Ok(()),
        Err(error) => {
            let current = unsafe { GetThreadDpiAwarenessContext() };
            if unsafe {
                AreDpiAwarenessContextsEqual(current, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            }
            .as_bool()
            {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

/// The portable tab state inside the shell state (`reentry::is_free_nested`).
fn portable_cell(state: &AppState) -> &RefCell<PortableAppState> {
    &state.portable
}

/// True when neither the shell state nor the portable state inside it is
/// borrowed: every entry point that can run reentrantly checks this and
/// defers its work otherwise (`reentry`).
fn shell_free(state: &RefCell<AppState>) -> bool {
    crate::reentry::is_free_nested(state, portable_cell)
}

unsafe fn app_state_handle(hwnd: HWND) -> Option<Rc<RefCell<AppState>>> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Rc<RefCell<AppState>>;
    if pointer.is_null() {
        None
    } else {
        // SAFETY: GWLP_USERDATA owns a boxed Rc from WM_NCCREATE until it is
        // cleared in WM_NCDESTROY. Cloning creates a scoped strong handle, so
        // nested dispatch cannot invalidate the allocation. No AppState
        // reference or RefCell borrow is returned from this function.
        Some(Rc::clone(unsafe { &*pointer }))
    }
}
unsafe fn set_user_data(hwnd: HWND, value: isize) -> Result<(), Error> {
    unsafe { SetLastError(WIN32_ERROR(0)) };
    let previous = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, value) };
    if previous == 0 && unsafe { GetLastError() } != WIN32_ERROR(0) {
        Err(Error::from_thread())
    } else {
        Ok(())
    }
}

fn close_state(state: &Rc<RefCell<AppState>>) {
    input::shutdown(state);
    let (mut session, mut accessibility, apartment, observer, controls) = {
        let mut state = state.borrow_mut();
        if state.destroyed {
            return;
        }
        state.destroyed = true;
        state.uia_selection.take();
        // chrome: cancel repository work.
        state.chrome.shutdown();
        // runtime: stop setup notifications and a running download.
        state.runtime.shutdown();
        (
            state.webview.take(),
            state.accessibility.take(),
            state.apartment.take(),
            state.page_observer.take(),
            state.controls.take(),
        )
    };
    if let Some(accessibility) = accessibility.as_mut() {
        accessibility.disconnect();
    }
    let error = session
        .as_mut()
        .and_then(|session| session.close().err())
        .map(|error| format!("WebView2 close: {error:?}"));
    drop(session);
    drop(observer);
    drop(apartment);
    style::clear_brushes();
    let mut state = state.borrow_mut();
    if error.is_some() {
        state.shell_error = error;
    }
    let _ = controls;
}
