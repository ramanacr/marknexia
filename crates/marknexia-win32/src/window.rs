//! Raw Win32 shell. All HWND and WebView2 COM state stays on the owning STA.

use std::{cell::RefCell, collections::BTreeMap, ffi::c_void, path::PathBuf, rc::Rc};

use marknexia_security::{ContentPolicy, HtmlPolicy};
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
            COLORREF, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, SetLastError,
            WIN32_ERROR, WPARAM,
        },
        Graphics::{
            Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute},
            Gdi::{
                BeginPaint, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
                COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateSolidBrush,
                DeleteObject, DrawFocusRect, EndPaint, FillRect, GetSysColor, GetSysColorBrush,
                HGDIOBJ, InvalidateRect, PAINTSTRUCT, SetBkMode, SetTextColor, TRANSPARENT,
                TextOutW, UpdateWindow,
            },
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        },
        UI::{
            HiDpi::{
                AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                GetDpiForWindow, GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_SHIFT},
            WindowsAndMessaging::{
                CREATESTRUCTW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
                DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW, GetWindowLongPtrW,
                IDC_ARROW, LoadCursorW, MSG, PostQuitMessage, RegisterClassW, SW_HIDE, SW_SHOW,
                SWP_NOACTIVATE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow,
                TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_DPICHANGED,
                WM_GETOBJECT, WM_KEYDOWN, WM_LBUTTONDOWN, WM_NCCREATE, WM_NCDESTROY, WM_PAINT,
                WM_SETTINGCHANGE, WM_SIZE, WM_SYSCOLORCHANGE, WM_THEMECHANGED, WNDCLASSW, WS_CHILD,
                WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
            },
        },
    },
    core::{Error, PCWSTR, w},
};

use crate::{
    accessibility::{
        native::{NativeAccessibility, SelectTab, is_uia_root_request},
        tab_slot,
    },
    app::{AppState as PortableAppState, FocusSurface},
    keyboard::{KeyChord, ShellCommand, route_key},
    layout::{PixelRect, ShellLayout, ShellLayoutRequest},
    tabs::TabId,
    theme::{ColorSpec, EffectiveTheme, SystemColorRole, palette, resolve_theme},
};

const CLASS_NAME: PCWSTR = w!("MarknexiaRustWindow");
const TAB_CLASS_NAME: PCWSTR = w!("MarknexiaRustTabStrip");
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
    last_error: Option<String>,
    dpi: u32,
    theme: EffectiveTheme,
    destroyed: bool,
}

impl AppState {
    fn new() -> Self {
        Self {
            portable: Rc::new(RefCell::new(PortableAppState::new())),
            controls: None,
            webview: None,
            apartment: None,
            page_observer: None,
            accessibility: None,
            uia_selection: None,
            webview_checked_out: false,
            last_error: None,
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
    if let Err(error) = initialize(hwnd, instance) {
        let _ = unsafe { DestroyWindow(hwnd) };
        return Err(error);
    }
    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
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
        let handled = msg.message == WM_KEYDOWN && route_native_key(hwnd, msg.wParam.0 as u16);
        if !handled {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        poll_webview(hwnd);
    }
    Ok(())
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

fn initialize(hwnd: HWND, instance: HINSTANCE) -> Result<(), WindowError> {
    let controls = ShellControls {
        command: create_child(
            hwnd,
            instance,
            w!("BUTTON"),
            w!("Open Markdown"),
            child_style(0),
            1001,
        )?,
        tabs: create_child(hwnd, instance, TAB_CLASS_NAME, w!(""), child_style(0), 1002)?,
        sidebar: create_child(hwnd, instance, w!("LISTBOX"), w!(""), child_style(1), 1003)?,
        find: create_child(
            hwnd,
            instance,
            w!("EDIT"),
            w!("Find"),
            child_style(0x80),
            1004,
        )?,
        status: create_child(
            hwnd,
            instance,
            w!("STATIC"),
            w!("Ready"),
            child_style(0),
            1005,
        )?,
    };
    let state = unsafe { app_state_handle(hwnd) }.ok_or(WindowError::State)?;
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let portable = {
        let mut state = state.borrow_mut();
        state.controls = Some(controls);
        state.dpi = dpi;
        Rc::clone(&state.portable)
    };
    {
        let mut app = portable.borrow_mut();
        let _ = app.open_tab("Welcome");
        let _ = app.open_tab("Security and privacy");
    }
    // UIA `Select` completes synchronously: it returns only after portable
    // state, the active WebView controller, and the tab strip agree. No shell
    // borrow spans the WebView COM call (see `select_tab_id`), so reentry from
    // UIA on this STA is safe.
    let select_callback: Rc<SelectTab> =
        Rc::new(move |tab_id: TabId| select_tab_id(hwnd, tab_id.get(), false));
    let accessibility = NativeAccessibility::new(
        controls.tabs,
        Rc::downgrade(&portable),
        Rc::downgrade(&select_callback),
    );
    {
        let mut state = state.borrow_mut();
        state.accessibility = Some(accessibility);
        state.uia_selection = Some(select_callback);
    }
    match StaApartment::enter() {
        Ok(apartment) => {
            // Until the session is stored, a reentrant UIA Select must not
            // report success against a controller it cannot reach.
            state.borrow_mut().webview_checked_out = true;
            let apartment = Rc::new(apartment);
            let observer: Rc<dyn Fn(PageToHost)> = Rc::new(|_| {});
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
            for (tab_id, title) in tabs {
                // Placeholder content until the rendering pipeline lands:
                // plain text only, through the sealed sanitized constructor.
                let document = HtmlPolicy::new(ContentPolicy::default())
                    .encode_text(&format!("{title}: Marknexia native document viewport."))
                    .map_err(|error| format!("{error}"))
                    .and_then(|body| {
                        HostDocument::new_titled(tab_id.get(), 1, &title, body, BTreeMap::new())
                            .map_err(|error| format!("{error:?}"))
                    });
                let added = document.and_then(|document| {
                    session
                        .add_document(document)
                        .map_err(|error| format!("{error:?}"))
                });
                if let Err(error) = added {
                    startup_error = Some(format!("WebView2 document: {error}"));
                }
            }
            if let Some(active) = active {
                let _ = session.select_tab(active.get());
            }
            match session.start() {
                Ok(()) => {
                    let mut state = state.borrow_mut();
                    state.webview_checked_out = false;
                    state.webview = Some(session);
                    state.apartment = Some(apartment);
                    state.page_observer = Some(observer);
                    state.last_error = startup_error;
                }
                Err(error) => {
                    let _ = session.close();
                    let mut state = state.borrow_mut();
                    state.webview_checked_out = false;
                    state.last_error = Some(format!("WebView2 unavailable: {error:?}"));
                }
            }
        }
        Err(error) => {
            state.borrow_mut().last_error = Some(format!("COM STA unavailable: {error:?}"))
        }
    }
    update_theme(hwnd);
    reflow(hwnd, dpi);
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
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
            let provider = state.borrow().accessibility.clone();
            provider.as_ref().map_or(LRESULT(0), |provider| unsafe {
                provider.return_provider(wparam, lparam)
            })
        }
        WM_LBUTTONDOWN => {
            if let Ok(parent) = unsafe { windows::Win32::UI::WindowsAndMessaging::GetParent(hwnd) }
            {
                select_tab_at(parent, (lparam.0 as i16) as i32);
                let _ = unsafe { SetFocus(Some(hwnd)) };
            }
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
    let (tabs, selected, focus_visible, colors, dpi) = {
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
        )
    };
    let count = tabs.len();
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
        fill(dc, &client, native_color(colors.surface));
        for (index, (tab_id, title)) in tabs.iter().enumerate() {
            if let Some((left, right)) = tab_slot(client.right - client.left, count, index) {
                let rect = RECT {
                    left,
                    top: 0,
                    right,
                    bottom: client.bottom,
                };
                let active = selected == Some(*tab_id);
                fill(
                    dc,
                    &rect,
                    native_color(if active {
                        colors.elevated
                    } else {
                        colors.surface
                    }),
                );
                unsafe {
                    SetTextColor(
                        dc,
                        native_color(if active { colors.accent } else { colors.text }),
                    );
                    SetBkMode(dc, TRANSPARENT);
                }
                let text: Vec<u16> = title.encode_utf16().collect();
                let x = rect
                    .left
                    .saturating_add((12_u32.saturating_mul(dpi) / 96) as i32);
                let y = rect
                    .top
                    .saturating_add((12_u32.saturating_mul(dpi) / 96) as i32);
                let _ = unsafe { TextOutW(dc, x, y, &text) };
                if active && focus_visible {
                    let _ = unsafe { DrawFocusRect(dc, &rect) };
                }
            }
        }
    }
    let _ = unsafe { EndPaint(hwnd, &paint) };
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
    if !ctrl && !shift && matches!(key, 0x25 | 0x27) {
        let tab_strip = unsafe { app_state_handle(hwnd) }
            .and_then(|state| state.borrow().controls.map(|controls| controls.tabs));
        if tab_strip.is_none() || Some(unsafe { GetFocus() }) != tab_strip {
            return false;
        }
    }
    route_key(KeyChord::new(key, ctrl, shift, false))
        .is_some_and(|command| execute_command(hwnd, command))
}

fn execute_command(hwnd: HWND, command: ShellCommand) -> bool {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return false;
    };
    let portable = Rc::clone(&state.borrow().portable);
    portable.borrow_mut().note_keyboard_input();
    if command == ShellCommand::CloseTab {
        let Some(active) = portable.borrow().active_tab() else {
            return false;
        };
        if let Some(mut session) = take_session(&state) {
            let result = session.remove_document(active.get());
            restore_session(&state, session);
            if result.is_err() {
                return false;
            }
        }
    }
    let previous = portable.borrow().active_tab();
    let applied = portable.borrow_mut().apply_command(command);
    if !applied {
        return false;
    }
    if command == ShellCommand::CycleFocus {
        let (controls, focus) = {
            let state = state.borrow();
            (state.controls, portable.borrow().focused_surface())
        };
        if let Some(controls) = controls {
            let target = focus_target(controls, focus);
            let _ = unsafe { SetFocus(Some(target)) };
        }
        refresh_tabs(&state, None);
        return true;
    }
    // Bind the active tab first: a borrow held in an `if let` scrutinee would
    // span the reentrant WebView COM call below.
    let active = portable.borrow().active_tab();
    let closed = command == ShellCommand::CloseTab;
    // Commands that keep the active tab (sidebar, find bar) need no switch.
    let switched = closed || active != previous;
    if switched
        && let Some(active) = active
        && !select_webview(&state, active.get())
    {
        // Keep portable state and the visible controller in agreement.
        if command != ShellCommand::CloseTab
            && let Some(previous) = previous
        {
            let _ = portable.borrow_mut().select_tab(previous);
        }
        refresh_tabs(&state, None);
        return false;
    }
    if closed {
        let a11y = state.borrow().accessibility.clone();
        if let Some(a11y) = a11y {
            a11y.notify_children_changed();
        }
    }
    refresh_tabs(&state, active.filter(|id| Some(*id) != previous));
    true
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
    let (portable, width) = {
        let state = state.borrow();
        (
            Rc::clone(&state.portable),
            state
                .accessibility
                .as_ref()
                .map_or(1, NativeAccessibility::tab_strip_width)
                .max(1),
        )
    };
    portable.borrow_mut().note_pointer_input();
    let count = portable.borrow().tabs().tabs().len();
    if count == 0 {
        return;
    }
    let width = i32::try_from(width).unwrap_or(i32::MAX);
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
    let (a11y, controls) = {
        let state = state.borrow();
        (state.accessibility.clone(), state.controls)
    };
    if let Some(c) = controls {
        unsafe {
            let _ = InvalidateRect(Some(c.tabs), None, true);
            let _ = UpdateWindow(c.tabs);
        }
    }
    if let (Some(a11y), Some(id)) = (a11y.as_ref(), newly_selected) {
        a11y.notify_selected(id);
    }
}

fn poll_webview(hwnd: HWND) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    let Some(mut session) = take_session(&state) else {
        return;
    };
    let error = session
        .poll()
        .err()
        .map(|error| format!("WebView2 poll: {error:?}"));
    restore_session(&state, session);
    if let Some(error) = error {
        state.borrow_mut().last_error = Some(error);
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
    let (portable, controls) = {
        let state = state.borrow();
        (Rc::clone(&state.portable), state.controls)
    };
    let actual_dpi = dpi.max(96);
    state.borrow_mut().dpi = actual_dpi;
    let app = portable.borrow();
    let layout = ShellLayout::compute(
        ShellLayoutRequest::new(
            client.right.max(0) as u32,
            client.bottom.max(0) as u32,
            actual_dpi,
        )
        .with_sidebar_visible(app.sidebar_visible())
        .with_find_bar_visible(app.find_bar_visible()),
    );
    drop(app);
    if let Some(c) = controls {
        for (window, rect) in [
            (c.command, layout.command_bar),
            (c.tabs, layout.tab_strip),
            (c.sidebar, layout.sidebar),
            (c.find, layout.find_bar),
            (c.status, layout.status_bar),
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
    let (preference, controls) = {
        let state = state.borrow();
        (state.portable.borrow().theme(), state.controls)
    };
    let theme = resolve_theme(preference, system_prefers_dark(), high_contrast());
    state.borrow_mut().theme = theme;
    apply_window_theme(hwnd, theme);
    if let Some(mut session) = take_session(&state) {
        let _ = session.set_background(webview_color(palette(theme).background));
        restore_session(&state, session);
    }
    if let Some(c) = controls {
        for window in [c.command, c.tabs, c.sidebar, c.find, c.status] {
            let _ = unsafe { InvalidateRect(Some(window), None, true) };
        }
    }
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

fn apply_window_theme(hwnd: HWND, theme: EffectiveTheme) {
    let dark = i32::from(matches!(theme, EffectiveTheme::Dark));
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const i32).cast::<c_void>(),
            std::mem::size_of_val(&dark) as u32,
        )
    };
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
    let error = session
        .select_tab(tab_id)
        .err()
        .map(|error| format!("WebView2 select: {error:?}"));
    restore_session(state, session);
    match error {
        Some(error) => {
            state.borrow_mut().last_error = Some(error);
            false
        }
        None => true,
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
    let (mut session, mut accessibility, apartment, observer, controls) = {
        let mut state = state.borrow_mut();
        if state.destroyed {
            return;
        }
        state.destroyed = true;
        state.uia_selection.take();
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
    let mut state = state.borrow_mut();
    state.last_error = error.or_else(|| state.last_error.take());
    let _ = controls;
}
