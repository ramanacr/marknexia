#![cfg(windows)]

use marknexia_webview::environment::{
    BrowserExit, BrowserExitKind, BrowserVersionProbe, EvergreenRuntime, RuntimeError,
    RuntimeStatus, StaApartment, WebViewEnvironment,
};
use marknexia_webview::host::{ProcessFailure, ViewportBounds};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_FAILED, COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL,
    COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    COREWEBVIEW2_PROCESS_FAILED_KIND_UTILITY_PROCESS_EXITED,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
};

#[test]
fn browser_exit_kind_classification_distinguishes_normal_shutdown_from_failure() {
    assert_eq!(
        BrowserExitKind::from(COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL),
        BrowserExitKind::Normal
    );
    assert_eq!(
        BrowserExitKind::from(COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_FAILED),
        BrowserExitKind::Failed
    );
}

#[test]
fn sta_guard_initializes_and_balances_com_on_its_thread() {
    std::thread::spawn(|| {
        let apartment = StaApartment::enter().expect("fresh test thread must enter STA");
        // A second compatible initialization must succeed while the guard lives.
        assert!(unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) } >= 0);
        unsafe { CoUninitialize() };
        assert!(unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) } < 0);
        drop(apartment);
        // Once the guard drops, the fresh thread must be free to enter MTA.
        assert!(unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) } >= 0);
        unsafe { CoUninitialize() };
    })
    .join()
    .unwrap();
}

#[test]
fn sta_guard_rejects_a_preexisting_mta() {
    std::thread::spawn(|| {
        assert!(unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) } >= 0);
        assert!(StaApartment::enter().is_err());
        unsafe { CoUninitialize() };
    })
    .join()
    .unwrap();
}

struct VersionReply(Result<&'static str, i32>);

impl BrowserVersionProbe for VersionReply {
    fn query_version(&self) -> Result<String, i32> {
        self.0
            .as_ref()
            .map(|version| (*version).to_owned())
            .map_err(|error| *error)
    }
}

#[test]
fn reports_installed_runtime_version_without_an_install_action() {
    assert_eq!(
        EvergreenRuntime::detect_with(&VersionReply(Ok("153.0.4234.32"))),
        Ok(RuntimeStatus::Available {
            version: "153.0.4234.32".to_owned(),
        })
    );
}

#[test]
fn reports_missing_runtime_for_loader_file_not_found() {
    assert_eq!(
        EvergreenRuntime::detect_with(&VersionReply(Err(0x8007_0002u32 as i32))),
        Ok(RuntimeStatus::Missing)
    );
}

#[test]
fn preserves_unexpected_loader_failure_as_typed_error() {
    assert_eq!(
        EvergreenRuntime::detect_with(&VersionReply(Err(0x8000_4005u32 as i32))),
        Err(RuntimeError::LoaderFailure(0x8000_4005u32 as i32))
    );
}

#[test]
fn does_not_treat_a_dev_edge_channel_as_evergreen() {
    assert_eq!(
        EvergreenRuntime::detect_with(&VersionReply(Ok("153.0.4234.32 Dev"))),
        Ok(RuntimeStatus::Missing)
    );
}

#[test]
fn rejects_an_empty_success_version_from_the_loader() {
    assert_eq!(
        EvergreenRuntime::detect_with(&VersionReply(Ok(""))),
        Err(RuntimeError::EmptyVersion)
    );
}

#[test]
fn process_failure_classification_preserves_recovery_relevant_kinds() {
    assert_eq!(
        ProcessFailure::from(COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED),
        ProcessFailure::BrowserExited
    );
    assert_eq!(
        ProcessFailure::from(COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED),
        ProcessFailure::RendererExited
    );
    assert_eq!(
        ProcessFailure::from(COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE),
        ProcessFailure::RendererUnresponsive
    );
    assert_eq!(
        ProcessFailure::from(COREWEBVIEW2_PROCESS_FAILED_KIND_UTILITY_PROCESS_EXITED),
        ProcessFailure::Other(4)
    );
}

#[test]
#[ignore = "native feasibility host must have Evergreen installed"]
fn native_loader_detects_installed_evergreen_without_downloading() {
    assert!(matches!(
        EvergreenRuntime::detect(),
        Ok(RuntimeStatus::Available { version }) if !version.is_empty()
    ));
}

#[test]
#[ignore = "native feasibility host must have Evergreen installed"]
fn native_shared_environment_completes_on_sta_without_blocking_the_host() {
    std::thread::spawn(|| {
        use std::{
            cell::RefCell,
            rc::Rc,
            time::{Duration, Instant},
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
        };

        let apartment = Rc::new(StaApartment::enter().unwrap());
        let folder = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("marknexia-webview-feasibility")
            .join(std::process::id().to_string());
        let completed = Rc::new(RefCell::new(None));
        let callback_state = Rc::clone(&completed);
        WebViewEnvironment::create_async(
            Rc::clone(&apartment),
            &folder,
            Box::new(move |result| {
                *callback_state.borrow_mut() = Some(result);
            }),
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        while completed.borrow().is_none() && Instant::now() < deadline {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
                unsafe { DispatchMessageW(&message) };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let environment = completed
            .borrow_mut()
            .take()
            .expect("WebView2 environment callback must complete within 15 seconds")
            .expect("installed Evergreen must create an environment");
        assert!(!environment.browser_version().unwrap().is_empty());
        drop(environment);
        drop(apartment);
    })
    .join()
    .unwrap();
}

#[test]
#[ignore = "native feasibility host must have Evergreen installed"]
fn native_two_controllers_apply_independent_bounds_and_visibility() {
    std::thread::spawn(|| {
        use std::{cell::RefCell, rc::Rc, time::{Duration, Instant}};
        use windows::{
            core::w,
            Win32::UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
                WINDOW_EX_STYLE, WS_OVERLAPPED,
            },
        };

        let apartment = Rc::new(StaApartment::enter().unwrap());
        let folder = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("marknexia-webview-feasibility")
            .join(std::process::id().to_string());
        let environment_result = Rc::new(RefCell::new(None));
        let callback_state = Rc::clone(&environment_result);
        WebViewEnvironment::create_async(
            Rc::clone(&apartment),
            &folder,
            Box::new(move |result| *callback_state.borrow_mut() = Some(result)),
        )
        .unwrap();
        pump_until(&environment_result, Duration::from_secs(15));
        let environment = environment_result.borrow_mut().take().unwrap().unwrap();

        let browser_exits = Rc::new(RefCell::new(Vec::<BrowserExit>::new()));
        let observed_exits = Rc::clone(&browser_exits);
        let exit_observer: Rc<dyn Fn(BrowserExit)> = Rc::new(move |event| {
            observed_exits.borrow_mut().push(event);
        });
        let mut exit_subscription = environment
            .observe_browser_exit(Rc::downgrade(&exit_observer))
            .unwrap();
        assert_eq!(Rc::strong_count(&exit_observer), 1);

        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(), w!("STATIC"), w!("Marknexia Native Probe"),
                WS_OVERLAPPED, 0, 0, 400, 300, None, None, None, None,
            )
        }
        .unwrap();
        let controller_result = Rc::new(RefCell::new(None));
        let callback_state = Rc::clone(&controller_result);
        environment
            .create_host_async(parent, Box::new(move |result| {
                *callback_state.borrow_mut() = Some(result);
            }))
            .unwrap();
        pump_until(&controller_result, Duration::from_secs(30));
        let mut host = controller_result.borrow_mut().take().unwrap().unwrap();
        let expected = ViewportBounds { left: 0, top: 0, right: 320, bottom: 220 };
        host.set_bounds(expected).unwrap();
        assert_eq!(host.bounds().unwrap(), expected);
        host.set_visible(true).unwrap();
        assert!(host.is_visible().unwrap());

        // A process-failure callback must not keep application state alive.
        // Both tab controllers may report the same browser exit; the pure
        // recovery coordinator is responsible for deduplicating it.
        let failures = Rc::new(RefCell::new(Vec::<ProcessFailure>::new()));
        let observed = Rc::clone(&failures);
        let observer: Rc<dyn Fn(ProcessFailure)> = Rc::new(move |failure| {
            observed.borrow_mut().push(failure);
        });
        host.observe_process_failures(Rc::downgrade(&observer)).unwrap();
        assert_eq!(Rc::strong_count(&observer), 1);

        let second_result = Rc::new(RefCell::new(None));
        let callback_state = Rc::clone(&second_result);
        environment
            .create_host_async(parent, Box::new(move |result| {
                *callback_state.borrow_mut() = Some(result);
            }))
            .unwrap();
        pump_until(&second_result, Duration::from_secs(30));
        let mut second_host = second_result.borrow_mut().take().unwrap().unwrap();
        second_host.observe_process_failures(Rc::downgrade(&observer)).unwrap();
        assert_eq!(Rc::strong_count(&observer), 1);
        let second_bounds = ViewportBounds { left: 20, top: 10, right: 280, bottom: 180 };
        second_host.set_bounds(second_bounds).unwrap();
        assert_eq!(second_host.bounds().unwrap(), second_bounds);
        second_host.set_visible(false).unwrap();
        assert!(!second_host.is_visible().unwrap());
        assert!(host.is_visible().unwrap_or_else(|error| {
            panic!("first controller failed after second was hidden: {error:?}; process failures: {:?}; browser exits: {:?}", failures.borrow(), browser_exits.borrow());
        }));
        host.set_visible(false).unwrap();
        second_host.set_visible(true).unwrap();
        assert!(!host.is_visible().unwrap());
        assert!(second_host.is_visible().unwrap());

        let stable_until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < stable_until && browser_exits.borrow().is_empty() {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
                unsafe { DispatchMessageW(&message) };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(browser_exits.borrow().is_empty(), "browser exited while both controllers were open: {:?}", browser_exits.borrow());
        assert!(failures.borrow().is_empty(), "process failures while both controllers were open: {:?}", failures.borrow());

        second_host.close().unwrap();
        host.close().unwrap();
        exit_subscription.close().unwrap();
        drop(exit_observer);
        drop(observer);
        unsafe { DestroyWindow(parent) }.unwrap();
        drop(second_host);
        drop(host);
        drop(environment);
        drop(apartment);

        // Keep the hidden parent message queue active until all controller
        // callbacks above have completed. No nested blocking COM pump is used.
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
            unsafe { DispatchMessageW(&message) };
        }
    })
    .join()
    .unwrap();
}

fn pump_until<T>(
    result: &std::rc::Rc<std::cell::RefCell<Option<T>>>,
    timeout: std::time::Duration,
) {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW};
    let deadline = std::time::Instant::now() + timeout;
    while result.borrow().is_none() && std::time::Instant::now() < deadline {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
            unsafe { DispatchMessageW(&message) };
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        result.borrow().is_some(),
        "native callback did not complete within {timeout:?}"
    );
}
