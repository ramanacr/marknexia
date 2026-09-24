//! STA lifetime foundation for native WebView2 COM objects.

use std::{
    os::windows::ffi::OsStrExt,
    path::Path,
    rc::{Rc, Weak},
};

use webview2_com::{
    BrowserProcessExitedEventHandler, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND, COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_FAILED,
        COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL, CreateCoreWebView2EnvironmentWithOptions,
        GetAvailableCoreWebView2BrowserVersionString, ICoreWebView2Environment,
        ICoreWebView2Environment5, ICoreWebView2EnvironmentOptions,
    },
};
use windows::{
    Win32::Foundation::HWND,
    core::{Interface, PCWSTR, PWSTR},
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

use crate::host::{HostError, WebViewHost};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    Available { version: String },
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeError {
    LoaderFailure(i32),
    EmptyVersion,
}

pub trait BrowserVersionProbe {
    fn query_version(&self) -> Result<String, i32>;
}

struct NativeBrowserVersionProbe;

impl BrowserVersionProbe for NativeBrowserVersionProbe {
    fn query_version(&self) -> Result<String, i32> {
        let mut returned_version = PWSTR::null();
        // SAFETY: null browser folder requests the installed runtime. The SDK
        // owns no callback here; it writes a CoTaskMem-allocated PWSTR, which
        // take_pwstr frees on both success and error. No COM object is retained.
        let result = unsafe {
            GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut returned_version)
        };
        let version = webview2_com::take_pwstr(returned_version);
        result.map(|()| version).map_err(|error| error.code().0)
    }
}

pub struct EvergreenRuntime;

impl EvergreenRuntime {
    pub fn detect() -> Result<RuntimeStatus, RuntimeError> {
        Self::detect_with(&NativeBrowserVersionProbe)
    }

    pub fn detect_with(probe: &impl BrowserVersionProbe) -> Result<RuntimeStatus, RuntimeError> {
        match probe.query_version() {
            Ok(version) if version.trim().is_empty() => Err(RuntimeError::EmptyVersion),
            Ok(version) if is_preview_channel(&version) => Ok(RuntimeStatus::Missing),
            Ok(version) => Ok(RuntimeStatus::Available { version }),
            Err(code) if code == 0x8007_0002u32 as i32 => Ok(RuntimeStatus::Missing),
            Err(code) => Err(RuntimeError::LoaderFailure(code)),
        }
    }
}

fn is_preview_channel(version: &str) -> bool {
    let lowercase = version.to_ascii_lowercase();
    ["beta", "dev", "canary"]
        .iter()
        .any(|channel| lowercase.contains(channel))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaError {
    ComInitializationFailed(i32),
}

/// Keep this guard alive until every controller, event handler, and callback
/// owned by the apartment has been removed and released. It cannot be sent to
/// another thread, so its successful initialization is balanced on the same STA.
#[derive(Debug)]
pub struct StaApartment {
    _thread_affinity: Rc<()>,
}

impl StaApartment {
    pub fn enter() -> Result<Self, StaError> {
        // SAFETY: the reserved pointer is null; COM initialization is scoped to
        // this current thread. No WebView2 callback or COM object exists yet.
        // Both S_OK and S_FALSE require exactly one CoUninitialize on this STA.
        let result = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if result < 0 {
            return Err(StaError::ComInitializationFailed(result));
        }
        Ok(Self {
            _thread_affinity: Rc::new(()),
        })
    }
}

impl Drop for StaApartment {
    fn drop(&mut self) {
        // SAFETY: Rc makes the guard !Send and !Sync, so Drop runs on the same
        // thread that successfully entered COM. The owning native environment
        // must release controllers and callback registrations before this guard.
        unsafe { CoUninitialize() };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentError {
    UserDataFolder,
    StartupFailed(i32),
    MissingEnvironment,
    CallFailed(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserExitKind {
    Normal,
    Failed,
    Other(i32),
}

impl From<COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND> for BrowserExitKind {
    fn from(kind: COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND) -> Self {
        match kind {
            COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL => Self::Normal,
            COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_FAILED => Self::Failed,
            other => Self::Other(other.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserExit {
    pub kind: BrowserExitKind,
    pub process_id: u32,
}

/// Owns one environment-level callback token on the browser's STA.
/// Close explicitly before replacing the environment; Drop is a fallback.
pub struct BrowserExitSubscription {
    environment: Option<ICoreWebView2Environment5>,
    token: i64,
    _apartment: Rc<StaApartment>,
}

impl BrowserExitSubscription {
    pub fn close(&mut self) -> Result<(), EnvironmentError> {
        let Some(environment) = self.environment.take() else {
            return Ok(());
        };
        // SAFETY: the token belongs to this environment and is removed on the
        // creating STA before the final interface reference is released.
        unsafe { environment.remove_BrowserProcessExited(self.token) }
            .map_err(|error| EnvironmentError::CallFailed(error.code().0))
    }
}

impl Drop for BrowserExitSubscription {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// The returned native environment is shared by controllers on one STA. Its
/// COM interface drops before the apartment guard due to field declaration
/// order, and the guard also stays alive while any clone exists.
#[derive(Clone)]
pub struct WebViewEnvironment {
    inner: ICoreWebView2Environment,
    _apartment: Rc<StaApartment>,
}

impl WebViewEnvironment {
    pub fn observe_browser_exit(
        &self,
        observer: Weak<dyn Fn(BrowserExit)>,
    ) -> Result<BrowserExitSubscription, EnvironmentError> {
        let environment = self
            .inner
            .cast::<ICoreWebView2Environment5>()
            .map_err(|error| EnvironmentError::CallFailed(error.code().0))?;
        let handler = BrowserProcessExitedEventHandler::create(Box::new(move |_, args| {
            if let (Some(args), Some(observer)) = (args, observer.upgrade()) {
                let mut kind = COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND::default();
                let mut process_id = 0;
                // SAFETY: event args are live for this STA callback. Both
                // outputs are local values and are not retained by COM.
                unsafe { args.BrowserProcessExitKind(&mut kind) }?;
                unsafe { args.BrowserProcessId(&mut process_id) }?;
                observer(BrowserExit {
                    kind: kind.into(),
                    process_id,
                });
            }
            Ok(())
        }));
        let mut token = 0;
        // SAFETY: COM retains the handler after this STA registration. The
        // returned subscription owns the exact token and removes it on close.
        unsafe { environment.add_BrowserProcessExited(&handler, &mut token) }
            .map_err(|error| EnvironmentError::CallFailed(error.code().0))?;
        Ok(BrowserExitSubscription {
            environment: Some(environment),
            token,
            _apartment: Rc::clone(&self._apartment),
        })
    }

    pub fn create_async(
        apartment: Rc<StaApartment>,
        user_data_folder: &Path,
        completed: Box<dyn FnOnce(Result<Self, EnvironmentError>)>,
    ) -> Result<(), EnvironmentError> {
        std::fs::create_dir_all(user_data_folder).map_err(|_| EnvironmentError::UserDataFolder)?;
        let folder_utf16: Vec<u16> = user_data_folder
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let folder = PCWSTR::from_raw(folder_utf16.as_ptr());
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |result, environment| {
                let outcome = match result {
                    Err(error) => Err(EnvironmentError::StartupFailed(error.code().0)),
                    Ok(()) => environment
                        .map(|inner| Self {
                            inner,
                            _apartment: apartment,
                        })
                        .ok_or(EnvironmentError::MissingEnvironment),
                };
                completed(outcome);
                Ok(())
            },
        ));
        // SAFETY: folder_utf16 remains live for the synchronous start call.
        // WebView2 retains the COM handler for its one-shot STA callback. The
        // callback owns only an Rc apartment guard and no mutable Rust borrow.
        unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                folder,
                None::<&ICoreWebView2EnvironmentOptions>,
                &handler,
            )
        }
        .map_err(|error| EnvironmentError::StartupFailed(error.code().0))
    }

    pub fn browser_version(&self) -> Result<String, EnvironmentError> {
        let mut returned_version = PWSTR::null();
        // SAFETY: this COM interface and caller are kept on the owning STA by
        // the Rc guard. The SDK allocates the returned pointer with CoTaskMem;
        // take_pwstr frees it before this method returns. No callback is invoked.
        let result = unsafe { self.inner.BrowserVersionString(&mut returned_version) };
        let version = webview2_com::take_pwstr(returned_version);
        result
            .map(|()| version)
            .map_err(|error| EnvironmentError::CallFailed(error.code().0))
    }

    pub fn create_host_async(
        &self,
        parent: HWND,
        completed: Box<dyn FnOnce(Result<WebViewHost, HostError>)>,
    ) -> Result<(), HostError> {
        let shared_environment = self.clone();
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |result, controller| {
                let outcome = match result {
                    Err(error) => Err(HostError::ControllerStartupFailed(error.code().0)),
                    Ok(()) => {
                        controller
                            .ok_or(HostError::MissingController)
                            .and_then(|controller| {
                                WebViewHost::from_controller(controller, shared_environment)
                            })
                    }
                };
                completed(outcome);
                Ok(())
            },
        ));
        // SAFETY: parent HWND is supplied by the owning Win32 shell and must
        // stay alive until callback completion. Handler owns one-shot state and
        // an environment clone; no mutable borrow spans the COM start call.
        unsafe { self.inner.CreateCoreWebView2Controller(parent, &handler) }
            .map_err(|error| HostError::ControllerStartupFailed(error.code().0))
    }
}
