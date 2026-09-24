//! Raw Win32 top-level shell foundation. UI state never escapes this module.

use std::{ffi::c_void, ptr::NonNull};

use windows::{
    Win32::{
        Foundation::{
            GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, SetLastError, WIN32_ERROR, WPARAM,
        },
        Graphics::Gdi::{COLOR_WINDOW, GetSysColorBrush},
        System::LibraryLoader::GetModuleHandleW,
        UI::HiDpi::{
            AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            GetDpiForWindow, GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
        },
        UI::WindowsAndMessaging::{
            CREATESTRUCTW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
            DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW, GetWindowLongPtrW,
            IDC_ARROW, LoadCursorW, MSG, PostQuitMessage, RegisterClassW, SW_SHOW, SWP_NOACTIVATE,
            SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
            WINDOW_EX_STYLE, WM_DESTROY, WM_DPICHANGED, WM_NCCREATE, WM_NCDESTROY, WM_SIZE,
            WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        },
    },
    core::{Error, PCWSTR, w},
};

use crate::{
    layout::{ShellLayout, ShellLayoutRequest},
    tabs::TabStore,
};

const CLASS_NAME: PCWSTR = w!("MarknexiaRustPreviewWindow");
const WINDOW_TITLE: PCWSTR = w!("Marknexia Rust Preview");

#[cfg(test)]
mod tests {
    use super::{app_state_pointer, layout_for_client};

    #[test]
    fn layout_for_client_uses_the_current_dpi() {
        let layout = layout_for_client(960, 640, 144);

        assert_eq!(layout.status_bar.height, 66);
        assert_eq!(layout.status_bar.width, 960);
    }

    #[test]
    fn cleared_user_data_has_no_app_state_pointer() {
        assert!(app_state_pointer(0).is_none());
    }
}

#[derive(Debug)]
pub enum WindowError {
    DpiAwareness(Error),
    StateUnavailable,
    Module(Error),
    Cursor(Error),
    RegisterClass(Error),
    Create(Error),
    StatusSurface(Error),
    MessageLoop(Error),
}

struct AppState {
    _tabs: TabStore,
    status_surface: Option<HWND>,
    last_native_layout_error: Option<Error>,
}

struct CreatePayload {
    state: Option<Box<AppState>>,
}

pub fn run() -> Result<(), WindowError> {
    enable_per_monitor_dpi_v2().map_err(WindowError::DpiAwareness)?;
    // SAFETY: a null module name selects this executable; no borrowed pointer
    // or callback escapes the call.
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(WindowError::Module)?;
    let instance = HINSTANCE(module.0);
    // SAFETY: IDC_ARROW is a system cursor resource; the returned handle is
    // shared and remains valid for the registered window class lifetime.
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(WindowError::Cursor)?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: CLASS_NAME,
        hCursor: cursor,
        // SAFETY: this is a shared system brush, not application-owned.
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        ..Default::default()
    };
    // SAFETY: class fields and callback are static or live for the entire
    // message loop. The process owns this registration until process exit.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(WindowError::RegisterClass(Error::from_thread()));
    }

    let mut payload = CreatePayload {
        state: Some(Box::new(AppState {
            _tabs: TabStore::new(),
            status_surface: None,
            last_native_layout_error: None,
        })),
    };
    // SAFETY: CreateWindowExW synchronously consumes the stack payload pointer
    // in WM_NCCREATE; that callback takes ownership of the boxed AppState.
    // If creation fails before WM_NCCREATE, payload drops the state here.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            WINDOW_TITLE,
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            960,
            640,
            None,
            None,
            Some(instance),
            Some((&mut payload as *mut CreatePayload).cast::<c_void>()),
        )
    }
    .map_err(WindowError::Create)?;
    // SAFETY: this native STATIC child belongs to the new top-level HWND. It
    // holds only fixed preview text and is destroyed with its parent window.
    let status_surface = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!(
                "Marknexia — Rust/Win32 development preview\r\n\r\nNative window: running\r\nTab state and keyboard routing: built, not yet connected to controls\r\nWebView2 lifecycle: under test\r\nDocument viewport not connected\r\nPackaging, accessibility, and parity: not complete"
            ),
            WS_CHILD | WS_VISIBLE,
            32,
            32,
            860,
            520,
            Some(window),
            None,
            Some(instance),
            None,
        )
    };
    let status_surface = match status_surface {
        Ok(status_surface) => status_surface,
        Err(error) => {
            // SAFETY: the parent HWND was created on this thread. Destroying it
            // synchronously releases its children and AppState via WM_NCDESTROY.
            let _ = unsafe { DestroyWindow(window) };
            return Err(WindowError::StatusSurface(error));
        }
    };
    // SAFETY: WM_NCCREATE installed the AppState for this live HWND. The child
    // belongs to that HWND and remains valid until its parent is destroyed.
    {
        let Some(state) = (unsafe { app_state_mut(window) }) else {
            let _ = unsafe { DestroyWindow(window) };
            return Err(WindowError::StateUnavailable);
        };
        state.status_surface = Some(status_surface);
    }
    reflow_status_surface(window, unsafe { GetDpiForWindow(window) });
    // SAFETY: the created HWND belongs to this thread. ShowWindow only changes
    // visibility and does not transfer ownership.
    let _ = unsafe { ShowWindow(window, SW_SHOW) };

    let mut message = MSG::default();
    loop {
        // SAFETY: MSG lives for the call; this thread owns the window queue.
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 == -1 {
            let error = Error::from_thread();
            // SAFETY: the HWND belongs to this thread; WM_NCDESTROY releases
            // the boxed state before returning from the failed loop.
            let _ = unsafe { DestroyWindow(window) };
            return Err(WindowError::MessageLoop(error));
        }
        if status.0 == 0 {
            break;
        }
        // SAFETY: the message was returned from this thread's queue and is
        // dispatched before the local MSG storage is reused.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCREATE => {
            // SAFETY: Win32 supplies CREATESTRUCTW during WM_NCCREATE. The
            // lpCreateParams pointer is the live stack payload from run(), and
            // exactly one Box is moved into GWLP_USERDATA on this owning thread.
            let creation = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            let payload = unsafe { &mut *(creation.lpCreateParams as *mut CreatePayload) };
            let Some(state) = payload.state.take() else {
                return LRESULT(0);
            };
            let state = Box::into_raw(state);
            // SAFETY: the pointer is retained only by this HWND on this thread;
            // WM_NCDESTROY clears it and reconstructs the Box exactly once.
            if let Err(error) = unsafe { set_window_user_data(window, state as isize) } {
                eprintln!("Marknexia Rust shell failed to store AppState: {error:?}");
                // SAFETY: storage failed, so ownership remains with the stack
                // payload that outlives this WM_NCCREATE callback.
                payload.state = Some(unsafe { Box::from_raw(state) });
                return LRESULT(0);
            }
            // A checked successful setter is the one-way ownership transfer.
            // This module exclusively owns GWLP_USERDATA for this window, so
            // WM_NCDESTROY will observe exactly this Box pointer or zero.
            // SAFETY: default processing initializes the non-client caption
            // from CREATESTRUCTW while our state pointer is already installed.
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        WM_DPICHANGED => {
            let Some(suggested) = (unsafe { (lparam.0 as *const RECT).as_ref() }) else {
                return LRESULT(0);
            };
            // SAFETY: WM_DPICHANGED supplies a suggested physical RECT whose
            // lifetime is this call. SetWindowPos applies it without changing
            // z-order or activation, as required by the DPI message contract.
            if let Err(error) = unsafe {
                SetWindowPos(
                    window,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right.saturating_sub(suggested.left),
                    suggested.bottom.saturating_sub(suggested.top),
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
            } {
                record_native_layout_error(window, "WM_DPICHANGED suggested RECT", error);
            }
            reflow_status_surface(window, dpi_from_wparam(wparam));
            LRESULT(0)
        }
        WM_SIZE => {
            // SAFETY: GetDpiForWindow reads the DPI associated with this live
            // HWND. WM_NCCREATE has already initialized its AppState.
            reflow_status_surface(window, unsafe { GetDpiForWindow(window) });
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: this is this top-level HWND's owning message thread.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // SAFETY: clear HWND storage before invoking DefWindowProcW, which
            // may dispatch messages. No Rust borrow spans that Win32 call.
            let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) };
            let state_cleared = state == 0
                || (unsafe { set_window_user_data(window, 0) }.is_ok()
                    && unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } == 0);
            if !state_cleared {
                eprintln!("Marknexia Rust shell could not safely clear AppState during teardown");
            }
            let result = unsafe { DefWindowProcW(window, message, wparam, lparam) };
            if state != 0 && state_cleared {
                // SAFETY: WM_NCCREATE stored this Box pointer and no other
                // callback frees it. Clear-before-drop prevents stale access.
                unsafe { drop(Box::from_raw(state as *mut AppState)) };
            }
            result
        }
        _ => {
            // SAFETY: unhandled messages follow the normal Win32 window
            // procedure contract; no Rust reference is held across dispatch.
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
    }
}

fn layout_for_client(client_width: u32, client_height: u32, dpi: u32) -> ShellLayout {
    ShellLayout::compute(ShellLayoutRequest::new(client_width, client_height, dpi))
}

fn enable_per_monitor_dpi_v2() -> Result<(), Error> {
    // SAFETY: process DPI awareness is selected before this module registers a
    // class or creates any HWND, so all native UI is born in this context.
    match unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        Ok(()) => Ok(()),
        Err(error) => {
            // A host may have established PMv2 before entering this adapter.
            // The API remains the primary policy; only that equivalent context
            // is accepted after a failed attempt, never a weaker/mixed context.
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

fn dpi_from_wparam(wparam: WPARAM) -> u32 {
    (wparam.0 & 0xffff) as u32
}

fn reflow_status_surface(window: HWND, dpi: u32) {
    // SAFETY: this helper runs only while the top-level HWND is live. It reads
    // the client rect, then moves an owned child HWND without retaining borrows
    // across Win32 calls.
    let mut client = RECT::default();
    if unsafe { GetClientRect(window, &mut client) }.is_err() {
        return;
    }
    let width = u32::try_from(client.right).unwrap_or_default();
    let height = u32::try_from(client.bottom).unwrap_or_default();
    let layout = layout_for_client(width, height, dpi);
    // SAFETY: WM_NCCREATE owns the pointer in GWLP_USERDATA until WM_NCDESTROY.
    // WM_SIZE and WM_DPICHANGED are handled before WM_NCDESTROY begins.
    let status_surface = {
        let Some(state) = (unsafe { app_state_mut(window) }) else {
            return;
        };
        let Some(status_surface) = state.status_surface else {
            return;
        };
        status_surface
    };
    let bounds = layout.webview;
    // SAFETY: status_surface is a child of window, owned by its AppState. The
    // layout bounds are client-relative physical pixels, clamped to i32 range.
    if let Err(error) = unsafe {
        SetWindowPos(
            status_surface,
            None,
            bounds.x.min(i32::MAX as u32) as i32,
            bounds.y.min(i32::MAX as u32) as i32,
            bounds.width.min(i32::MAX as u32) as i32,
            bounds.height.min(i32::MAX as u32) as i32,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    } {
        record_native_layout_error(window, "status surface SetWindowPos", error);
    }
}

fn app_state_pointer(user_data: isize) -> Option<NonNull<AppState>> {
    NonNull::new(user_data as *mut AppState)
}

unsafe fn app_state_mut(window: HWND) -> Option<&'static mut AppState> {
    // SAFETY: callers only invoke this before WM_NCDESTROY. WM_NCCREATE stores
    // a single Box<AppState> pointer in this HWND and WM_NCDESTROY clears it
    // before reconstructing and dropping the Box.
    let pointer = app_state_pointer(unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) })?;
    Some(unsafe { pointer.as_ptr().as_mut().expect("NonNull pointer") })
}

unsafe fn set_window_user_data(window: HWND, value: isize) -> Result<(), Error> {
    // SAFETY: SetWindowLongPtrW reports zero both for a valid previous zero and
    // for failure. Microsoft requires clearing last error before the call.
    unsafe { SetLastError(WIN32_ERROR(0)) };
    let previous = unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, value) };
    if previous == 0 && unsafe { GetLastError() } != WIN32_ERROR(0) {
        Err(Error::from_thread())
    } else {
        Ok(())
    }
}

fn record_native_layout_error(window: HWND, operation: &str, error: Error) {
    eprintln!("Marknexia Rust shell {operation} failed: {error:?}");
    // SAFETY: teardown clears GWLP_USERDATA before nested destruction messages.
    // A missing state therefore turns this into a diagnostic-only no-op.
    if let Some(state) = unsafe { app_state_mut(window) } {
        state.last_native_layout_error = Some(error);
    }
}
