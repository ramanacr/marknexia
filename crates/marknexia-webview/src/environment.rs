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

use crate::{
    host::{HostError, WebViewHost},
    panic_flag::PanicFlag,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    Available { version: String },
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeError {
    LoaderFailure(i32),
}

pub trait BrowserVersionProbe {
    fn query_version(&self) -> Result<String, i32>;
}

/// `GetAvailableCoreWebView2BrowserVersionString` with the default browser
/// folder. The loader also honors `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER`.
pub struct NativeBrowserVersionProbe;

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

    /// A null or empty version means Evergreen is unavailable, like a
    /// missing file; preview channels do not count as the shared runtime.
    pub fn detect_with<P: BrowserVersionProbe + ?Sized>(
        probe: &P,
    ) -> Result<RuntimeStatus, RuntimeError> {
        match probe.query_version() {
            Ok(version) if version.trim().is_empty() => Ok(RuntimeStatus::Missing),
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
    /// The environment-created callback panicked; the panic was recorded.
    CallbackPanicked,
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
    /// The flag the callback records into (kept for tests).
    #[cfg_attr(not(test), allow(dead_code))]
    panics: PanicFlag,
    _apartment: Rc<StaApartment>,
}

impl BrowserExitSubscription {
    #[cfg(test)]
    pub(crate) fn panic_flag_for_test(&self) -> &PanicFlag {
        &self.panics
    }

    pub fn close(&mut self) -> Result<(), EnvironmentError> {
        let Some(environment) = self.environment.as_ref() else {
            return Ok(());
        };
        // SAFETY: the token belongs to this environment and is removed on the
        // creating STA before the final interface reference is released.
        unsafe { environment.remove_BrowserProcessExited(self.token) }
            .map_err(|error| EnvironmentError::CallFailed(error.code().0))?;
        self.environment.take();
        Ok(())
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
    pub(crate) fn native_environment(&self) -> &ICoreWebView2Environment {
        &self.inner
    }
    /// Observes browser-process exit. A panic caught in the callback is
    /// recorded in `panics` (the owning session's flag).
    pub fn observe_browser_exit(
        &self,
        observer: Weak<dyn Fn(BrowserExit)>,
        panics: &PanicFlag,
    ) -> Result<BrowserExitSubscription, EnvironmentError> {
        let environment = self
            .inner
            .cast::<ICoreWebView2Environment5>()
            .map_err(|error| EnvironmentError::CallFailed(error.code().0))?;
        let callback_panics = panics.clone();
        let handler = BrowserProcessExitedEventHandler::create(Box::new(move |_, args| {
            // The whole body runs under the guard; a panic is recorded and
            // surfaced by the next session poll.
            callback_panics
                .call(|| {
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
                })
                .unwrap_or(Ok(()))
        }));
        let mut token = 0;
        // SAFETY: COM retains the handler after this STA registration. The
        // returned subscription owns the exact token and removes it on close.
        unsafe { environment.add_BrowserProcessExited(&handler, &mut token) }
            .map_err(|error| EnvironmentError::CallFailed(error.code().0))?;
        Ok(BrowserExitSubscription {
            environment: Some(environment),
            token,
            panics: panics.clone(),
            _apartment: Rc::clone(&self._apartment),
        })
    }

    /// Starts environment creation. The completion handler runs under the
    /// panic guard: a panic while building the result delivers
    /// `Err(CallbackPanicked)` to `completed`, and a panic inside `completed`
    /// is recorded in `panics`.
    pub fn create_async(
        apartment: Rc<StaApartment>,
        user_data_folder: &Path,
        panics: &PanicFlag,
        completed: Box<dyn FnOnce(Result<Self, EnvironmentError>)>,
    ) -> Result<(), EnvironmentError> {
        let panics = panics.clone();
        std::fs::create_dir_all(user_data_folder).map_err(|_| EnvironmentError::UserDataFolder)?;
        let folder_utf16: Vec<u16> = user_data_folder
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let folder = PCWSTR::from_raw(folder_utf16.as_ptr());
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |result, environment| {
                let outcome = panics
                    .call(|| match result {
                        Err(error) => Err(EnvironmentError::StartupFailed(error.code().0)),
                        Ok(()) => environment
                            .map(|inner| Self {
                                inner,
                                _apartment: apartment,
                            })
                            .ok_or(EnvironmentError::MissingEnvironment),
                    })
                    .unwrap_or(Err(EnvironmentError::CallbackPanicked));
                panics.guard(|| completed(outcome));
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

    /// Starts controller creation. The completion handler runs under the
    /// panic guard like [`Self::create_async`]:
    /// - if building the host panics, the controller is closed before it is
    ///   released and `completed` receives `Err(CallbackPanicked)`;
    /// - if `completed` itself panics, `lost` runs (under the guard) so the
    ///   caller can still record a failed result instead of waiting forever.
    pub fn create_host_async(
        &self,
        parent: HWND,
        panics: &PanicFlag,
        completed: Box<dyn FnOnce(Result<WebViewHost, HostError>)>,
        lost: Box<dyn FnOnce()>,
    ) -> Result<(), HostError> {
        let shared_environment = self.clone();
        let panics = panics.clone();
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |result, controller| {
                // Kept so a panic while building the host can still close
                // the controller before it is released.
                let close_on_panic = controller.clone();
                let outcome =
                    panics
                        .call(|| match result {
                            Err(error) => Err(HostError::ControllerStartupFailed(error.code().0)),
                            Ok(()) => controller.ok_or(HostError::MissingController).and_then(
                                |controller| {
                                    WebViewHost::from_controller(controller, shared_environment)
                                },
                            ),
                        })
                        .unwrap_or_else(|| {
                            if let Some(controller) = close_on_panic.as_ref() {
                                // SAFETY: the controller belongs to this STA
                                // callback; Close is its documented teardown.
                                let _ = panics.call(|| unsafe { controller.Close() });
                            }
                            Err(HostError::CallbackPanicked)
                        });
                drop(close_on_panic);
                if panics.call(|| completed(outcome)).is_none() {
                    panics.guard(lost);
                }
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
