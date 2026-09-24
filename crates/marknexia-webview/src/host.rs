//! One native controller and its CoreWebView2 interface on the owning STA.

use std::rc::Weak;

use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE, ICoreWebView2,
        ICoreWebView2Controller,
    },
    ProcessFailedEventHandler,
};
use windows::{Win32::Foundation::RECT, core::BOOL};

use crate::environment::WebViewEnvironment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    ControllerStartupFailed(i32),
    MissingController,
    MissingWebView(i32),
    CallFailed(i32),
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessFailure {
    BrowserExited,
    RendererExited,
    RendererUnresponsive,
    Other(i32),
}

impl From<COREWEBVIEW2_PROCESS_FAILED_KIND> for ProcessFailure {
    fn from(kind: COREWEBVIEW2_PROCESS_FAILED_KIND) -> Self {
        match kind {
            COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => Self::BrowserExited,
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => Self::RendererExited,
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
                Self::RendererUnresponsive
            }
            other => Self::Other(other.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportBounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl From<ViewportBounds> for RECT {
    fn from(value: ViewportBounds) -> Self {
        Self {
            left: value.left,
            top: value.top,
            right: value.right,
            bottom: value.bottom,
        }
    }
}

impl From<RECT> for ViewportBounds {
    fn from(value: RECT) -> Self {
        Self {
            left: value.left,
            top: value.top,
            right: value.right,
            bottom: value.bottom,
        }
    }
}

/// The parent HWND is owned by the Win32 shell. Call `close` before destroying
/// it; Drop also closes defensively. This type is !Send via its environment.
pub struct WebViewHost {
    core: Option<ICoreWebView2>,
    controller: Option<ICoreWebView2Controller>,
    process_failure_tokens: Vec<i64>,
    _environment: WebViewEnvironment,
}

impl WebViewHost {
    pub(crate) fn from_controller(
        controller: ICoreWebView2Controller,
        environment: WebViewEnvironment,
    ) -> Result<Self, HostError> {
        // SAFETY: the controller callback executes on the creating STA. The
        // environment clone retains that apartment, and the returned COM
        // interface is owned by this host until close/drop, not by a callback.
        let core = unsafe { controller.CoreWebView2() }
            .map_err(|error| HostError::MissingWebView(error.code().0))?;
        Ok(Self {
            core: Some(core),
            controller: Some(controller),
            process_failure_tokens: Vec::new(),
            _environment: environment,
        })
    }

    pub fn set_bounds(&self, bounds: ViewportBounds) -> Result<(), HostError> {
        let controller = self.controller.as_ref().ok_or(HostError::Closed)?;
        // SAFETY: the controller and caller are on the environment's STA. RECT
        // is copied by the COM call; no Rust mutable borrow crosses dispatch.
        unsafe { controller.SetBounds(bounds.into()) }
            .map_err(|error| HostError::CallFailed(error.code().0))
    }

    pub fn bounds(&self) -> Result<ViewportBounds, HostError> {
        let controller = self.controller.as_ref().ok_or(HostError::Closed)?;
        let mut bounds = RECT::default();
        // SAFETY: COM writes only into this live local RECT on the owning STA.
        // There is no callback retaining the pointer after the call returns.
        unsafe { controller.Bounds(&mut bounds) }
            .map_err(|error| HostError::CallFailed(error.code().0))?;
        Ok(bounds.into())
    }

    pub fn set_visible(&self, visible: bool) -> Result<(), HostError> {
        let controller = self.controller.as_ref().ok_or(HostError::Closed)?;
        // SAFETY: visibility is copied into the COM controller on its owning
        // STA. The host retains no borrowed Rust state through this call.
        unsafe { controller.SetIsVisible(visible) }
            .map_err(|error| HostError::CallFailed(error.code().0))
    }

    pub fn is_visible(&self) -> Result<bool, HostError> {
        let controller = self.controller.as_ref().ok_or(HostError::Closed)?;
        let mut visible = BOOL::default();
        // SAFETY: COM writes a BOOL into this live local on its owning STA;
        // no callback retains the pointer after return.
        unsafe { controller.IsVisible(&mut visible) }
            .map_err(|error| HostError::CallFailed(error.code().0))?;
        Ok(visible.as_bool())
    }

    /// Register a process-failure listener without creating a COM-to-app
    /// reference cycle. The application retains the strong observer reference.
    pub fn observe_process_failures(
        &mut self,
        observer: Weak<dyn Fn(ProcessFailure)>,
    ) -> Result<(), HostError> {
        let core = self.core.as_ref().ok_or(HostError::Closed)?;
        let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
            if let (Some(args), Some(observer)) = (args, observer.upgrade()) {
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                // SAFETY: the event args are owned for this STA callback. The
                // output is a live stack value and is not retained by COM.
                unsafe { args.ProcessFailedKind(&mut kind) }?;
                observer(ProcessFailure::from(kind));
            }
            Ok(())
        }));
        let mut token = 0;
        // SAFETY: the core and callback are used on their creating STA. COM
        // retains the handler after registration; the token is kept until
        // explicit removal before this controller is closed.
        unsafe { core.add_ProcessFailed(&handler, &mut token) }
            .map_err(|error| HostError::CallFailed(error.code().0))?;
        self.process_failure_tokens.push(token);
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), HostError> {
        let mut first_error = None;
        if let Some(core) = self.core.as_ref() {
            for token in self.process_failure_tokens.drain(..) {
                // SAFETY: remove every retained event registration on the
                // creating STA before releasing CoreWebView2 or controller.
                if let Err(error) = unsafe { core.remove_ProcessFailed(token) } {
                    first_error.get_or_insert(HostError::CallFailed(error.code().0));
                }
            }
        }
        self.core.take();
        let Some(controller) = self.controller.take() else {
            return first_error.map_or(Ok(()), Err);
        };
        // SAFETY: release the page interface first, then explicitly close the
        // controller on its STA before the owning Win32 shell destroys HWND.
        // Event registrations must be removed here before they are added later.
        if let Err(error) = unsafe { controller.Close() } {
            first_error.get_or_insert(HostError::CallFailed(error.code().0));
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for WebViewHost {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
