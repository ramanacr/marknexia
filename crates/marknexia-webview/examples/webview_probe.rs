#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{rc::Rc, time::Duration};

    use marknexia_webview::{
        environment::{EvergreenRuntime, RuntimeStatus, StaApartment},
        host::ViewportBounds,
        probe,
        protocol::PageToHost,
        session::WebViewSession,
    };
    use windows::{
        Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, IsWindow, MSG, PM_REMOVE,
            PeekMessageW, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        },
        core::w,
    };

    let apartment = Rc::new(StaApartment::enter().map_err(|error| probe_error("STA", error))?);
    match EvergreenRuntime::detect().map_err(|error| probe_error("Evergreen", error))? {
        RuntimeStatus::Missing => {
            eprintln!(
                "WebView2 Evergreen is absent. The shell can remain open; install only after user action at https://developer.microsoft.com/en-us/microsoft-edge/webview2/"
            );
            return Ok(());
        }
        RuntimeStatus::Available { version } => println!("WebView2 Evergreen {version}"),
    }

    // SAFETY: STA exists before HWND creation. This system STATIC class is
    // solely an interactive feasibility parent; its handle is destroyed after
    // every controller and event registration has been closed.
    let parent = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!("Marknexia WebView2 probe — close to exit"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            100,
            100,
            900,
            650,
            None,
            None,
            None,
            None,
        )
    }?;
    let observer: Rc<dyn Fn(PageToHost)> = Rc::new(|message| println!("page: {message:?}"));
    let folder = std::env::temp_dir()
        .join("marknexia-webview-probe")
        .join(std::process::id().to_string());
    let mut session = WebViewSession::new(
        Rc::clone(&apartment),
        parent,
        folder,
        Rc::downgrade(&observer),
    );
    session
        .add_document(probe::document(7, 1))
        .map_err(|error| probe_error("tab 7", error))?;
    session
        .add_document(probe::document(8, 1))
        .map_err(|error| probe_error("tab 8", error))?;
    session
        .select_tab(7)
        .map_err(|error| probe_error("selection", error))?;
    session
        .start()
        .map_err(|error| probe_error("environment", error))?;

    let mut laid_out = false;
    let mut should_close = false;
    while !should_close && unsafe { IsWindow(parent) }.as_bool() {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
            if message.hwnd == parent
                && message.message == windows::Win32::UI::WindowsAndMessaging::WM_CLOSE
            {
                should_close = true;
                continue;
            }
            unsafe { DispatchMessageW(&message) };
        }
        session
            .poll()
            .map_err(|error| probe_error("session", error))?;
        if !laid_out {
            if let (Some(first), Some(second)) = (session.host(7), session.host(8)) {
                first
                    .set_bounds(ViewportBounds {
                        left: 0,
                        top: 0,
                        right: 880,
                        bottom: 600,
                    })
                    .map_err(|error| probe_error("first bounds", error))?;
                second
                    .set_bounds(ViewportBounds {
                        left: 0,
                        top: 0,
                        right: 880,
                        bottom: 600,
                    })
                    .map_err(|error| probe_error("second bounds", error))?;
                first
                    .set_visible(true)
                    .map_err(|error| probe_error("first visibility", error))?;
                second
                    .set_visible(false)
                    .map_err(|error| probe_error("second visibility", error))?;
                laid_out = true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    session
        .close()
        .map_err(|error| probe_error("close", error))?;
    // SAFETY: IsWindow is false if the user already destroyed this HWND.
    if unsafe { IsWindow(parent) }.as_bool() {
        unsafe { DestroyWindow(parent) }?;
    }
    drop(session);
    drop(apartment);
    Ok(())
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn probe_error(context: &str, error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{context}: {error:?}"))
}
