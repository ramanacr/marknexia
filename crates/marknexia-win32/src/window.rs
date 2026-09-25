//! Raw Win32 shell. All HWND and WebView2 COM state stays on the owning STA.

use std::{cell::RefCell, collections::BTreeMap, ffi::c_void, path::PathBuf, rc::Rc};

use marknexia_webview::{
    environment::StaApartment,
    host::{HostColor, ViewportBounds},
    policy::HostDocument,
    protocol::PageToHost,
    session::WebViewSession,
};
use windows::{
    core::{w, Error, PCWSTR},
    Win32::{
        Foundation::{
            GetLastError, SetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WIN32_ERROR, WPARAM,
        },
        Graphics::{
            Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE},
            Gdi::{
                BeginPaint, CreateSolidBrush, DeleteObject, DrawFocusRect, EndPaint, FillRect,
                GetSysColor, GetSysColorBrush, SetBkMode, SetTextColor, TextOutW, COLORREF,
                COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
                COLOR_WINDOW, COLOR_WINDOWTEXT, HGDIOBJ, PAINTSTRUCT, TRANSPARENT,
            },
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
        },
        UI::{
            HiDpi::{
                AreDpiAwarenessContextsEqual, GetDpiForWindow, GetThreadDpiAwarenessContext,
                SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
                GetMessageW, GetWindowLongPtrW, InvalidateRect, LoadCursorW, PostMessageW,
                PostQuitMessage, RegisterClassW, SetFocus, SetWindowLongPtrW, SetWindowPos,
                ShowWindow, TranslateMessage, CREATESTRUCTW, CW_USEDEFAULT, GWLP_USERDATA,
                IDC_ARROW, MSG, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW, WINDOW_EX_STYLE,
                WINDOW_STYLE, WM_APP, WM_DESTROY, WM_DPICHANGED, WM_GETOBJECT, WM_KEYDOWN,
                WM_LBUTTONDOWN, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_SETTINGCHANGE, WM_SIZE,
                WM_SYSCOLORCHANGE, WM_THEMECHANGED, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW,
                WS_TABSTOP, WS_VISIBLE,
            },
        },
    },
};

use crate::{
    accessibility::native::{is_uia_root_request, NativeAccessibility},
    app::{AppState as PortableAppState, FocusSurface},
    keyboard::{route_key, KeyChord, ShellCommand},
    layout::{PixelRect, ShellLayout, ShellLayoutRequest},
    theme::{palette, resolve_theme, ColorSpec, EffectiveTheme, SystemColorRole},
};

const CLASS_NAME: PCWSTR = w!("MarknexiaRustWindow");
const TAB_CLASS_NAME: PCWSTR = w!("MarknexiaRustTabStrip");
const WINDOW_TITLE: PCWSTR = w!("Marknexia");
const WM_SELECT_TAB: u32 = WM_APP + 1;

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
    uia_selection: Option<Rc<dyn Fn(crate::tabs::TabId)>>,
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
            last_error: None,
            dpi: 96,
            theme: EffectiveTheme::Light,
            destroyed: false,
        }
    }
}

struct CreatePayload {
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
    let select_callback: Rc<dyn Fn(crate::tabs::TabId)> = Rc::new(move |tab_id| {
        // Queue selection onto the shell loop. The UIA callback never borrows
        // shell state or calls WebView2 while COM is still on its stack.
        let _ = unsafe {
            PostMessageW(
                Some(hwnd),
                WM_SELECT_TAB,
                WPARAM(tab_id.get() as usize),
                LPARAM(0),
            )
        };
    });
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
                    app.active_id(),
                )
            };
            let mut startup_error = None;
            for (tab_id, title) in tabs {
                let html = format!("<!doctype html><html><body><main><h1>{title}</h1><p>Marknexia native document viewport.</p></main></body></html>").into_bytes();
                if let Err(error) = session.add_document(HostDocument {
                    tab_id: tab_id.get(),
                    document_epoch: 1,
                    html,
                    assets: BTreeMap::new(),
                }) {
                    startup_error = Some(format!("WebView2 document: {error:?}"));
                }
            }
            if let Some(active) = active {
                let _ = session.select_tab(active.get());
            }
            match session.start() {
                Ok(()) => {
                    let mut state = state.borrow_mut();
                    state.webview = Some(session);
                    state.apartment = Some(apartment);
                    state.page_observer = Some(observer);
                    state.last_error = startup_error;
                }
                Err(error) => {
                    let _ = session.close();
                    state.borrow_mut().last_error =
                        Some(format!("WebView2 unavailable: {error:?}"));
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
        WM_SELECT_TAB => {
            select_tab_id(hwnd, wparam.0 as u64, false);
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
        if count > 0 {
            let width = (client.right - client.left).max(0) / count as i32;
            for (index, (tab_id, title)) in tabs.iter().enumerate() {
                let mut rect = RECT {
                    left: index as i32 * width,
                    top: 0,
                    right: if index + 1 == count {
                        client.right
                    } else {
                        (index as i32 + 1) * width
                    },
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
    unsafe { EndPaint(hwnd, &paint) };
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
        ColorSpec::System(role) => unsafe {
            GetSysColor(match role {
                SystemColorRole::Window => COLOR_WINDOW,
                SystemColorRole::WindowText => COLOR_WINDOWTEXT,
                SystemColorRole::Highlight => COLOR_HIGHLIGHT,
                SystemColorRole::HighlightText => COLOR_HIGHLIGHTTEXT,
                SystemColorRole::ButtonFace => COLOR_BTNFACE,
                SystemColorRole::ButtonText => COLOR_BTNTEXT,
                SystemColorRole::GrayText => COLOR_GRAYTEXT,
            })
        },
    }
}

fn route_native_key(hwnd: HWND, key: u16) -> bool {
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    if !ctrl && !shift && matches!(key, 0x25 | 0x27) {
        let tab_strip = unsafe { app_state_handle(hwnd) }
            .and_then(|state| state.borrow().controls.map(|controls| controls.tabs));
        if tab_strip.is_none()
            || unsafe { windows::Win32::UI::WindowsAndMessaging::GetFocus() } != tab_strip
        {
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
    if !portable.borrow_mut().apply_command(command) {
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
    } else if let Some(active) = portable.borrow().active_tab() {
        select_webview(&state, active.get());
    }
    refresh_tabs(&state);
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
    let index =
        ((x.max(0) as u32).saturating_mul(count as u32) / width).min(count as u32 - 1) as usize;
    let id = portable.borrow().tabs().tabs()[index].id();
    select_tab_id(hwnd, id.get(), true);
}

fn select_tab_id(hwnd: HWND, raw_id: u64, pointer_input: bool) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    let portable = Rc::clone(&state.borrow().portable);
    let id = portable
        .borrow()
        .tabs()
        .tabs()
        .iter()
        .find(|tab| tab.id().get() == raw_id)
        .map(|tab| tab.id());
    let Some(id) = id else { return };
    if pointer_input {
        portable.borrow_mut().note_pointer_input();
    }
    if portable.borrow_mut().select_tab(id) {
        select_webview(&state, id.get());
        refresh_tabs(&state);
    }
}

fn refresh_tabs(state: &Rc<RefCell<AppState>>) {
    let (a11y, controls) = {
        let state = state.borrow();
        (state.accessibility.clone(), state.controls)
    };
    if let Some(a11y) = a11y.as_ref() {
        a11y.refresh();
    }
    if let Some(c) = controls {
        let _ = unsafe { InvalidateRect(Some(c.tabs), None, true) };
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
    state.borrow_mut().webview.take()
}
fn restore_session(state: &Rc<RefCell<AppState>>, session: WebViewSession) {
    let mut state = state.borrow_mut();
    if !state.destroyed {
        state.webview = Some(session);
    }
}
fn select_webview(state: &Rc<RefCell<AppState>>, tab_id: u64) {
    if let Some(mut session) = take_session(state) {
        let error = session
            .select_tab(tab_id)
            .err()
            .map(|error| format!("WebView2 select: {error:?}"));
        restore_session(state, session);
        if let Some(error) = error {
            state.borrow_mut().last_error = Some(error);
        }
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
    drop(controls);
}
