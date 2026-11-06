//! The WebView2 missing-runtime state in the document area
//! (`packaging/webview/README.md`, decision record
//! `compat/decisions/webview2-missing-runtime.md`).
//!
//! When `GetAvailableCoreWebView2BrowserVersionString` reports no Evergreen
//! runtime, no WebView session is created. The shell stays usable and the
//! document area shows an explanation, an "Install WebView2 Runtime" button
//! and a link to Microsoft's offline installer page; the diagnostics bar
//! says the same. The install runs only after the user presses the button:
//! a worker thread downloads, verifies, runs and detects
//! (`marknexia_webview::bootstrap::Bootstrap`), posting progress back here.
//! A failure keeps the shell usable, shows the offline link and is never
//! retried automatically. When the runtime is found, the WebView session
//! starts and the open documents render again.

use std::{
    cell::RefCell,
    ffi::c_void,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use marknexia_core::contracts::DiagnosticSeverity;
use marknexia_webview::{
    bootstrap::{
        Bootstrap, DownloadLimits, MISSING_DIAGNOSTIC, OFFLINE_INSTALLER_PAGE, RuntimeSetup,
        SetupFailure, SetupPhase, SetupStep, TempStager, failure_message,
    },
    bootstrap_native::{AuthenticodeVerifier, ProcessRunner, WinHttpDownloader},
    environment::{EvergreenRuntime, NativeBrowserVersionProbe, RuntimeStatus},
};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{DeleteObject, HFONT, HGDIOBJ},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{ICC_LINK_CLASS, INITCOMMONCONTROLSEX, InitCommonControlsEx, NMHDR},
            Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus},
            Shell::ShellExecuteW,
            WindowsAndMessaging::{
                GW_HWNDPREV, GetWindow, GetWindowThreadProcessId, IsWindow, PostMessageW,
                SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, WINDOW_STYLE,
                WM_APP, WM_COMMAND, WM_NOTIFY, WS_CHILD, WS_TABSTOP,
            },
        },
    },
    core::{PCWSTR, w},
};

use super::{AppState, WindowError, app_state_handle, chrome, create_child};
use crate::layout::PixelRect;

pub(super) const ID_RUNTIME_TEXT: usize = 2030;
pub(super) const ID_RUNTIME_INSTALL: usize = 2031;
pub(super) const ID_RUNTIME_CANCEL: usize = 2032;
pub(super) const ID_RUNTIME_LINK: usize = 2033;
/// Wakes the message loop when the setup worker queued an event; the loop
/// tail applies it (`poll`), never a nested pump.
const WM_RUNTIME_EVENT: u32 = WM_APP + 0x50;

const INSTALL_LABEL: &str = "Install WebView2 Runtime";
const CANCEL_LABEL: &str = "Cancel download";
const LINK_TEXT: &str = "Get Microsoft's offline installer";
const NM_CLICK: u32 = 0xFFFF_FFFE;
const NM_RETURN: u32 = 0xFFFF_FFFC;
const SS_NOPREFIX: u32 = 0x80;
const BS_PUSHBUTTON: u32 = 0;
const WM_SETFONT: u32 = 0x0030;

enum Event {
    Step(u64, SetupStep),
    Done(u64, Result<String, SetupFailure>),
}

#[derive(Clone, Copy)]
struct Controls {
    text: HWND,
    install: HWND,
    cancel: HWND,
    link: HWND,
}

/// Missing-runtime state inside the shell's `AppState`.
pub(super) struct RuntimeUi {
    setup: RuntimeSetup,
    /// The missing-runtime state is shown (no WebView session exists).
    active: bool,
    controls: Option<Controls>,
    cancel: Option<Arc<AtomicBool>>,
    /// Cleared at WM_DESTROY: the worker stops posting.
    alive: Arc<AtomicBool>,
    sender: mpsc::Sender<Event>,
    events: mpsc::Receiver<Event>,
    font: Option<(u32, HFONT)>,
    bounds: PixelRect,
    dpi: u32,
}

impl RuntimeUi {
    pub(super) fn new() -> Self {
        let (sender, events) = mpsc::channel();
        Self {
            setup: RuntimeSetup::new(),
            active: false,
            controls: None,
            cancel: None,
            alive: Arc::new(AtomicBool::new(true)),
            sender,
            events,
            font: None,
            bounds: PixelRect::default(),
            dpi: 96,
        }
    }

    /// Stops the worker's notifications and asks a running download to
    /// stop (WM_DESTROY). A running installer is not stopped.
    pub(super) fn shutdown(&mut self) {
        self.alive.store(false, Ordering::Release);
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
    }
}

impl Drop for RuntimeUi {
    fn drop(&mut self) {
        if let Some((_, font)) = self.font.take() {
            // SAFETY: the font was created for these controls and is not
            // deleted elsewhere; the controls are gone or no longer paint.
            let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
        }
    }
}

/// True only when the loader reports no Evergreen runtime. A loader
/// failure is left to the session start, which reports it as before.
pub(super) fn runtime_missing() -> bool {
    matches!(EvergreenRuntime::detect(), Ok(RuntimeStatus::Missing))
}

/// Enters the missing-runtime state: no session, a clear message in the
/// document area and the diagnostics bar, and the install action.
pub(super) fn show_missing(hwnd: HWND, state: &Rc<RefCell<AppState>>) {
    let existing = state.borrow().runtime.controls;
    let controls = match existing {
        Some(controls) => Ok(controls),
        None => create(hwnd),
    };
    {
        let mut state = state.borrow_mut();
        if let Ok(controls) = controls {
            state.runtime.controls = Some(controls);
        }
        state.runtime.active = true;
        state.shell_error = Some("WebView2 unavailable: runtime missing".to_owned());
        let signal = state.startup.webview_unavailable();
        state.apply_startup(signal);
        state
            .chrome
            .diagnostic(MISSING_DIAGNOSTIC, DiagnosticSeverity::Warning);
    }
    sync(state);
}

fn create(parent: HWND) -> Result<Controls, WindowError> {
    // SAFETY: the module handle of this process; no ownership.
    let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map(|module| HINSTANCE(module.0))
        .map_err(WindowError::Module)?;
    let style = |extra: u32| WINDOW_STYLE(WS_CHILD.0 | extra);
    let text = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!(""),
        style(SS_NOPREFIX),
        ID_RUNTIME_TEXT as isize,
    )?;
    let install = create_child(
        parent,
        instance,
        w!("BUTTON"),
        w!("Install WebView2 Runtime"),
        style(WS_TABSTOP.0 | BS_PUSHBUTTON),
        ID_RUNTIME_INSTALL as isize,
    )?;
    let cancel = create_child(
        parent,
        instance,
        w!("BUTTON"),
        w!("Cancel download"),
        style(WS_TABSTOP.0 | BS_PUSHBUTTON),
        ID_RUNTIME_CANCEL as isize,
    )?;
    let link = create_link(parent, instance)?;
    let controls = Controls {
        text,
        install,
        cancel,
        link,
    };
    order(parent, &controls);
    Ok(controls)
}

/// A SysLink (UIA hyperlink). Without Common Controls 6 (no manifest) it
/// falls back to a push button with the same command.
fn create_link(parent: HWND, instance: HINSTANCE) -> Result<HWND, WindowError> {
    let classes = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_LINK_CLASS,
    };
    // SAFETY: registers the SysLink class; the struct is a live local.
    let _ = unsafe { InitCommonControlsEx(&classes) };
    let markup: Vec<u16> = format!("<a>{LINK_TEXT}</a>")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    create_child(
        parent,
        instance,
        w!("SysLink"),
        PCWSTR::from_raw(markup.as_ptr()),
        WINDOW_STYLE(WS_CHILD.0 | WS_TABSTOP.0),
        ID_RUNTIME_LINK as isize,
    )
    .or_else(|_| {
        create_child(
            parent,
            instance,
            w!("BUTTON"),
            w!("Get Microsoft's offline installer"),
            WINDOW_STYLE(WS_CHILD.0 | WS_TABSTOP.0 | BS_PUSHBUTTON),
            ID_RUNTIME_LINK as isize,
        )
    })
}

/// Tab order: the document area comes before the find bar.
fn order(parent: HWND, controls: &Controls) {
    let state = unsafe { app_state_handle(parent) };
    let find = state.and_then(|state| state.try_borrow().ok().and_then(|s| s.controls));
    let Some(find) = find.map(|shell| shell.find) else {
        return;
    };
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    // SAFETY: z-order changes among this window's own children.
    unsafe {
        let mut after = GetWindow(find, GW_HWNDPREV).ok();
        for window in [
            controls.text,
            controls.install,
            controls.cancel,
            controls.link,
        ] {
            let _ = SetWindowPos(window, after, 0, 0, 0, 0, flags);
            after = Some(window);
        }
    }
}

/// Places the state in the document area (window.rs `reflow`).
pub(super) fn layout(state: &Rc<RefCell<AppState>>, bounds: PixelRect, dpi: u32) {
    if let Ok(mut state) = state.try_borrow_mut() {
        state.runtime.bounds = bounds;
        state.runtime.dpi = dpi;
    } else {
        return;
    }
    sync(state);
}

/// The window messages of this module; `None` leaves the message to
/// window.rs.
pub(super) fn handle_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    match message {
        // Only wakes the loop; `poll` runs at the loop tail.
        WM_RUNTIME_EVENT => Some(LRESULT(0)),
        WM_COMMAND if (wparam.0 >> 16) & 0xffff == 0 => match wparam.0 & 0xffff {
            ID_RUNTIME_INSTALL => {
                begin_install(hwnd);
                Some(LRESULT(0))
            }
            ID_RUNTIME_CANCEL => {
                request_cancel(hwnd);
                Some(LRESULT(0))
            }
            // The fallback button.
            ID_RUNTIME_LINK => {
                open_offline_page(hwnd);
                Some(LRESULT(0))
            }
            _ => None,
        },
        WM_NOTIFY => {
            // SAFETY: for WM_NOTIFY, lParam points to an NMHDR (or a larger
            // structure that starts with one) valid for this message.
            let header = unsafe { (lparam.0 as *const NMHDR).as_ref() }?;
            if header.idFrom == ID_RUNTIME_LINK && matches!(header.code, NM_CLICK | NM_RETURN) {
                open_offline_page(hwnd);
                return Some(LRESULT(0));
            }
            None
        }
        _ => None,
    }
}

/// Opens Microsoft's offline installer page in the default browser, for a
/// click or Enter on the link only.
fn open_offline_page(hwnd: HWND) {
    let url: Vec<u16> = OFFLINE_INSTALLER_PAGE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: NUL-terminated constant URL that outlives the call.
    let _ = unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("open"),
            PCWSTR::from_raw(url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
}

/// The explicit user action: starts one attempt on a worker thread.
fn begin_install(hwnd: HWND) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    let started = {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let ui = &mut state.runtime;
        if !ui.active {
            return;
        }
        ui.setup.begin_install().map(|attempt| {
            let cancel = Arc::new(AtomicBool::new(false));
            ui.cancel = Some(Arc::clone(&cancel));
            (attempt, cancel, ui.sender.clone(), Arc::clone(&ui.alive))
        })
    };
    let Some((attempt, cancel, sender, alive)) = started else {
        return;
    };
    let window = hwnd.0 as isize;
    let spawned = std::thread::Builder::new()
        .name("marknexia-webview2-setup".to_owned())
        .spawn(move || {
            let post = |event: Event| deliver(window, &alive, &sender, event);
            let limits = DownloadLimits::default();
            let stager = TempStager::new(limits);
            let runner = ProcessRunner::default();
            let bootstrap = Bootstrap {
                downloader: &WinHttpDownloader,
                stager: &stager,
                verifier: &AuthenticodeVerifier,
                runner: &runner,
                probe: &NativeBrowserVersionProbe,
                limits,
            };
            let result = bootstrap.run(&cancel, &mut |step| post(Event::Step(attempt, step)));
            post(Event::Done(attempt, result));
        });
    if spawned.is_err() {
        let mut state = state.borrow_mut();
        state.runtime.setup.finish(
            attempt,
            Err(SetupFailure::Install(
                marknexia_webview::bootstrap::InstallError::StartFailed(0),
            )),
        );
    }
    sync(&state);
}

/// Queues `event` and wakes the shell's loop, unless the window is gone.
fn deliver(window: isize, alive: &AtomicBool, sender: &mpsc::Sender<Event>, event: Event) {
    if !alive.load(Ordering::Acquire) || sender.send(event).is_err() {
        return;
    }
    let window = HWND(window as *mut c_void);
    let mut owner = 0;
    // SAFETY: plain window queries and a post without pointers; the alive
    // flag, IsWindow and the owning process guard against a recycled handle.
    unsafe {
        if !IsWindow(Some(window)).as_bool() {
            return;
        }
        GetWindowThreadProcessId(window, Some(&mut owner));
        if owner == std::process::id() {
            let _ = PostMessageW(Some(window), WM_RUNTIME_EVENT, WPARAM(0), LPARAM(0));
        }
    }
}

fn request_cancel(hwnd: HWND) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let ui = &mut state.runtime;
        if ui.setup.request_cancel()
            && let Some(cancel) = ui.cancel.as_ref()
        {
            cancel.store(true, Ordering::Release);
        }
    }
    sync(&state);
}

/// Applies the worker's events (message-loop tail, no borrow held).
pub(super) fn poll(hwnd: HWND) {
    let Some(state) = (unsafe { app_state_handle(hwnd) }) else {
        return;
    };
    let events: Vec<Event> = match state.try_borrow() {
        Ok(state) if !state.webview_checked_out => state.runtime.events.try_iter().collect(),
        _ => return,
    };
    if events.is_empty() {
        return;
    }
    let mut installed = None;
    {
        let mut state = state.borrow_mut();
        for event in events {
            match event {
                Event::Step(attempt, step) => {
                    state.runtime.setup.progress(attempt, step);
                }
                Event::Done(attempt, outcome) => {
                    if !state.runtime.setup.finish(attempt, outcome) {
                        continue;
                    }
                    state.runtime.cancel = None;
                    match state.runtime.setup.phase().clone() {
                        SetupPhase::Installed { version } => installed = Some(version),
                        SetupPhase::Failed(failure) => {
                            let severity = match failure {
                                SetupFailure::Cancelled => DiagnosticSeverity::Warning,
                                _ => DiagnosticSeverity::Error,
                            };
                            state.chrome.diagnostic(failure_message(&failure), severity);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if let Some(version) = installed {
        super::start_after_install(hwnd);
        let mut state = state.borrow_mut();
        state.runtime.active = false;
        state.chrome.diagnostic(
            format!("WebView2 Runtime {version} is installed."),
            DiagnosticSeverity::Info,
        );
    }
    sync(&state);
}

/// Shows, hides, places and labels the controls for the current phase.
fn sync(state: &Rc<RefCell<AppState>>) {
    let Ok(mut borrowed) = state.try_borrow_mut() else {
        return;
    };
    let ui = &mut borrowed.runtime;
    let Some(controls) = ui.controls else {
        return;
    };
    let dpi = ui.dpi.max(96);
    let refont = ui.font.is_none_or(|(font_dpi, _)| font_dpi != dpi);
    let replaced = if refont {
        ui.font.replace((dpi, super::toolbar::text_font(dpi)))
    } else {
        None
    };
    let font = ui.font.map(|(_, font)| font);
    let visible = ui.active && ui.bounds.width > 0 && ui.bounds.height > 0;
    let message = ui.setup.message();
    let show_install = visible && ui.setup.can_install();
    let show_cancel = visible && ui.setup.is_working();
    let can_cancel = ui.setup.can_cancel();
    let show_link = visible && ui.setup.shows_offline_link();
    let bounds = ui.bounds;
    drop(borrowed);

    let windows = [
        controls.text,
        controls.install,
        controls.cancel,
        controls.link,
    ];
    if let Some(font) = font {
        for window in windows {
            chrome::send(window, WM_SETFONT, font.0 as usize, 1);
        }
    }
    if let Some((_, old)) = replaced {
        // SAFETY: every control now uses the new font.
        let _ = unsafe { DeleteObject(HGDIOBJ(old.0)) };
    }
    // Focus on a control that is about to hide moves to its replacement.
    // SAFETY: focus query on this thread.
    let focus = unsafe { GetFocus() };
    let dip = |value: u32| chrome::dip(value, dpi);
    let margin = dip(24);
    let width = bounds.width.saturating_sub(2 * margin).min(dip(640));
    let x = bounds.x.saturating_add(margin);
    let mut y = bounds.y.saturating_add(margin);
    let text_height = dip(96).min(bounds.height.saturating_sub(margin));
    chrome::set_bounds(
        controls.text,
        PixelRect {
            x,
            y,
            width,
            height: text_height,
        },
    );
    y = y.saturating_add(text_height).saturating_add(dip(12));
    let button = PixelRect {
        x,
        y,
        width: dip(240).min(width),
        height: dip(32),
    };
    chrome::set_bounds(controls.install, button);
    chrome::set_bounds(controls.cancel, button);
    y = y.saturating_add(dip(32)).saturating_add(dip(16));
    chrome::set_bounds(
        controls.link,
        PixelRect {
            x,
            y,
            width,
            height: dip(24),
        },
    );
    set_text(controls.text, &message);
    set_text(controls.install, INSTALL_LABEL);
    set_text(controls.cancel, CANCEL_LABEL);
    // SAFETY: enables or disables this thread's own control.
    let _ = unsafe { EnableWindow(controls.cancel, can_cancel) };
    chrome::show(controls.text, visible);
    chrome::show(controls.install, show_install);
    chrome::show(controls.cancel, show_cancel);
    chrome::show(controls.link, show_link);
    let target = if focus == controls.install && !show_install {
        show_cancel.then_some(controls.cancel)
    } else if focus == controls.cancel && (!show_cancel || !can_cancel) {
        show_install.then_some(controls.install)
    } else {
        None
    };
    if let Some(target) = target {
        // SAFETY: focuses this thread's own visible control.
        let _ = unsafe { SetFocus(Some(target)) };
    }
}

fn set_text(window: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated buffer that outlives the call; no borrow held.
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
            window,
            PCWSTR::from_raw(wide.as_ptr()),
        )
    };
}
