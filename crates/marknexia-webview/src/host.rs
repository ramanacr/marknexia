//! One native controller and its CoreWebView2 interface on the owning STA.

use std::{
    cell::RefCell,
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::{Rc, Weak},
};

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

use crate::{
    callbacks::CallbackTokens,
    environment::WebViewEnvironment,
    policy::HostDocument,
    protocol::{HostToPage, PageToHost, serialize_host_message},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    ControllerStartupFailed(i32),
    MissingController,
    MissingWebView(i32),
    CallFailed(i32),
    InvalidDocument,
    InvalidMessage,
    AlreadyBound,
    Closed,
}

impl HostError {
    pub(crate) fn from_com(error: windows::core::Error) -> Self {
        Self::CallFailed(error.code().0)
    }
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
    callbacks: Option<CallbackTokens>,
    document: Option<Rc<HostDocument>>,
    page_messages: Rc<RefCell<VecDeque<PageToHost>>>,
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
            callbacks: None,
            document: None,
            page_messages: Rc::new(RefCell::new(VecDeque::new())),
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
                let _ = catch_unwind(AssertUnwindSafe(|| observer(ProcessFailure::from(kind))));
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

    /// The immutable document is retained for the full controller lifetime.
    /// Every event sink receives a weak reference and has no path to raw COM
    /// from the application observer. A failed binding closes this controller.
    pub fn bind_document(&mut self, document: HostDocument) -> Result<(), HostError> {
        if self.callbacks.is_some() {
            return Err(HostError::AlreadyBound);
        }
        document
            .validate()
            .map_err(|_| HostError::InvalidDocument)?;
        let core = self.core.as_ref().ok_or(HostError::Closed)?.clone();
        let document = Rc::new(document);
        self.callbacks = Some(CallbackTokens::new(&core));
        self.document = Some(Rc::clone(&document));
        if let Err(error) = self.callbacks.as_mut().expect("just installed").register(
            self.controller.as_ref().ok_or(HostError::Closed)?,
            self._environment.native_environment(),
            &document,
            Rc::downgrade(&self.page_messages),
        ) {
            // Retain failed removal tokens in the host if cleanup itself
            // fails. The session can retry `close` before releasing COM.
            let _ = self.close();
            return Err(error);
        }
        let uri = document.document_uri();
        let uri: Vec<u16> = uri.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: the URI remains live for this synchronous COM call. The
        // registered resource handler supplies exactly the immutable document.
        if let Err(error) = unsafe { core.Navigate(windows::core::PCWSTR::from_raw(uri.as_ptr())) }
        {
            let _ = self.close();
            return Err(HostError::from_com(error));
        }
        Ok(())
    }

    pub fn document_identity(&self) -> Result<(u64, u64), HostError> {
        let document = self.document.as_ref().ok_or(HostError::Closed)?;
        Ok((document.tab_id, document.document_epoch))
    }

    /// Only a typed, identity-bound message can cross into the page. The
    /// immutable document epoch makes a stale renderer response rejectable.
    pub fn post_message(&self, message: &HostToPage) -> Result<(), HostError> {
        let core = self.core.as_ref().ok_or(HostError::Closed)?;
        let document = self.document.as_ref().ok_or(HostError::InvalidDocument)?;
        if message.identity() != (1, document.tab_id, document.document_epoch) {
            return Err(HostError::InvalidMessage);
        }
        let json = serialize_host_message(message).map_err(|_| HostError::InvalidMessage)?;
        let json: Vec<u16> = json.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { core.PostWebMessageAsJson(windows::core::PCWSTR::from_raw(json.as_ptr())) }
            .map_err(HostError::from_com)
    }

    pub(crate) fn resource_boundary_failed(&self) -> bool {
        self.callbacks
            .as_ref()
            .is_some_and(CallbackTokens::resource_failed)
    }

    /// Drain validated page messages after Windows dispatch, outside COM.
    pub fn drain_page_messages(&self) -> Vec<PageToHost> {
        self.page_messages.borrow_mut().drain(..).collect()
    }

    pub fn close(&mut self) -> Result<(), HostError> {
        let mut first_error = None;
        if let Some(callbacks) = self.callbacks.as_mut() {
            callbacks.close()?;
        }
        if let Some(core) = self.core.as_ref() {
            let mut failed_tokens = Vec::new();
            for token in self.process_failure_tokens.drain(..) {
                // SAFETY: remove every retained event registration on the
                // creating STA before releasing CoreWebView2 or controller.
                if let Err(error) = unsafe { core.remove_ProcessFailed(token) } {
                    first_error.get_or_insert(HostError::CallFailed(error.code().0));
                    failed_tokens.push(token);
                }
            }
            self.process_failure_tokens = failed_tokens;
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        if let Some(controller) = self.controller.as_ref() {
            // An uncompleted resource deferral, if any, remains held inside
            // callbacks until Close succeeds. This prevents a request with no
            // explicit response from being released to network fallback.
            unsafe { controller.Close() }.map_err(HostError::from_com)?;
        }
        self.controller.take();
        self.core.take();
        self.callbacks.take();
        self.document.take();
        self.page_messages.borrow_mut().clear();
        Ok(())
    }

    /// A resource-boundary failure must close the controller even if event
    /// token removal fails. Once Close succeeds, WebView2 can no longer issue
    /// callbacks and failed removal tokens no longer need retry ownership.
    pub(crate) fn abort_security_boundary(&mut self) -> Result<(), HostError> {
        if let Some(callbacks) = self.callbacks.as_mut() {
            let _ = callbacks.close();
        }
        if let Some(core) = self.core.as_ref() {
            for token in self.process_failure_tokens.drain(..) {
                let _ = unsafe { core.remove_ProcessFailed(token) };
            }
        }
        if let Some(controller) = self.controller.as_ref() {
            unsafe { controller.Close() }.map_err(HostError::from_com)?;
        }
        self.controller.take();
        self.core.take();
        self.callbacks.take();
        self.document.take();
        self.page_messages.borrow_mut().clear();
        Ok(())
    }
}

impl Drop for WebViewHost {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
