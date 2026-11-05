//! WebView2 event sinks. Every callback uses immutable policy and weak app state.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    ffi::c_void,
    rc::{Rc, Weak},
};

use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
        ICoreWebView2, ICoreWebView2Deferral, ICoreWebView2Environment,
        ICoreWebView2WebResourceRequestedEventArgs, ICoreWebView2WebResourceResponse,
    },
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler,
};
use windows::{
    Win32::{
        Foundation::{E_UNEXPECTED, HGLOBAL},
        System::Com::{STREAM_SEEK_SET, StructuredStorage::CreateStreamOnHGlobal},
    },
    core::{PCWSTR, PWSTR},
};

use crate::{
    broker::{BrokerRequest, ResourceKind},
    host::HostError,
    panic_flag::PanicFlag,
    policy::{HostDocument, ResponseSpec},
    protocol::PageToHost,
};

pub(crate) struct CallbackTokens {
    core: ICoreWebView2,
    resource: Option<i64>,
    navigation: Option<i64>,
    navigation_completed: Option<i64>,
    message: Option<i64>,
    new_window: Option<i64>,
    filter: bool,
    resource_failed: Rc<Cell<bool>>,
    document_loaded: Rc<Cell<bool>>,
    /// The pinned document's navigation completed without success before
    /// it ever loaded.
    navigation_failed: Rc<Cell<bool>>,
    pending_deferrals: Rc<RefCell<Vec<ICoreWebView2Deferral>>>,
}

impl CallbackTokens {
    pub(crate) fn new(core: &ICoreWebView2) -> Self {
        Self {
            core: core.clone(),
            resource: None,
            navigation: None,
            navigation_completed: None,
            message: None,
            new_window: None,
            filter: false,
            resource_failed: Rc::new(Cell::new(false)),
            document_loaded: Rc::new(Cell::new(false)),
            navigation_failed: Rc::new(Cell::new(false)),
            pending_deferrals: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub(crate) fn register(
        &mut self,
        security_abort: Weak<dyn Fn() -> windows::core::Result<()>>,
        environment: &ICoreWebView2Environment,
        document: &Rc<HostDocument>,
        messages: Weak<RefCell<VecDeque<PageToHost>>>,
        panics: &PanicFlag,
    ) -> Result<(), HostError> {
        let settings = unsafe { self.core.Settings() }.map_err(HostError::from_com)?;
        for operation in [
            unsafe { settings.SetAreDevToolsEnabled(false) },
            unsafe { settings.SetAreHostObjectsAllowed(false) },
            unsafe { settings.SetAreDefaultScriptDialogsEnabled(false) },
            unsafe { settings.SetAreDefaultContextMenusEnabled(false) },
            unsafe { settings.SetIsWebMessageEnabled(true) },
        ] {
            operation.map_err(HostError::from_com)?;
        }

        let filter = wide("*");
        unsafe {
            self.core.AddWebResourceRequestedFilter(
                PCWSTR::from_raw(filter.as_ptr()),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            )
        }
        .map_err(HostError::from_com)?;
        self.filter = true;

        let weak_document = Rc::downgrade(document);
        let response_environment = environment.clone();
        // Allocate a controller-owned fallback before the first navigation.
        // If allocation fails, registration fails and this controller must
        // never navigate. The empty body remains safe to reuse.
        let deny_response = create_response(
            environment,
            ResponseSpec {
                body: b"",
                ..ResponseSpec::forbidden()
            },
        )
        .map_err(HostError::from_com)?;
        let resource_failed = Rc::clone(&self.resource_failed);
        let pending_deferrals = Rc::clone(&self.pending_deferrals);
        let resource_panics = panics.clone();
        let resource_handler =
            WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                // Fail closed: mark the boundary failed and close the
                // controller synchronously so an unanswered request cannot
                // fall through to the network.
                let fail_closed = || {
                    resource_failed.set(true);
                    if let Some(abort) = security_abort.upgrade() {
                        let _ = abort();
                    }
                };
                // The gate is owned out here, outside both guards: whatever
                // panics, its deferral is never released before Close.
                let mut held: Option<ResourceGate<ICoreWebView2Deferral>> = None;
                let outcome = resource_panics.call(|| {
                    let deferral = match unsafe { args.GetDeferral() } {
                        Ok(deferral) => deferral,
                        Err(_) => {
                            // No deferral exists to hold. Try an immediate
                            // explicit deny, then force controller teardown if
                            // that also fails.
                            return match unsafe { args.SetResponse(&deny_response) } {
                                Ok(()) => Ok(()),
                                Err(error) => {
                                    fail_closed();
                                    Err(error)
                                }
                            };
                        }
                    };
                    let gate = held.insert(ResourceGate::new(deferral));
                    // A panic while deciding the response is caught here and
                    // fails closed; the gate stays in `held`.
                    resource_panics
                        .call(|| {
                            let assignment = install_response(
                                gate,
                                || {
                                    response_decision(&args, weak_document.upgrade().as_deref())
                                        .and_then(|response| {
                                            create_response(&response_environment, response)
                                        })
                                        .and_then(|response| unsafe { args.SetResponse(&response) })
                                },
                                || unsafe { args.SetResponse(&deny_response) },
                            );
                            if assignment.is_err() {
                                // Keep the deferral alive and close immediately.
                                fail_closed();
                            }
                            let completion = gate
                                .complete_if_responded(|deferral| unsafe { deferral.Complete() });
                            if completion.is_err() {
                                fail_closed();
                            }
                            completion
                        })
                        .unwrap_or_else(|| {
                            fail_closed();
                            Err(E_UNEXPECTED.into())
                        })
                });
                let outcome = outcome.unwrap_or_else(|| {
                    fail_closed();
                    Err(E_UNEXPECTED.into())
                });
                if let Some(deferral) = held.as_mut().and_then(ResourceGate::take_pending) {
                    // WebView2 documents that an outstanding deferral blocks
                    // this request. Keep it live until controller.Close has
                    // succeeded; never complete it with no explicit response.
                    // ManuallyDrop: if anything below panics (caught by the
                    // guard), the deferral leaks instead of being released.
                    let deferral = std::mem::ManuallyDrop::new(deferral);
                    resource_panics.guard(|| match pending_deferrals.try_borrow_mut() {
                        Ok(mut pending) => {
                            pending.push(std::mem::ManuallyDrop::into_inner(deferral));
                        }
                        // Cannot store it: never release it either.
                        Err(_) => fail_closed(),
                    });
                }
                outcome
            }));
        let mut token = 0;
        unsafe {
            self.core
                .add_WebResourceRequested(&resource_handler, &mut token)
        }
        .map_err(HostError::from_com)?;
        self.resource = Some(token);

        let weak_document = Rc::downgrade(document);
        let navigation_panics = panics.clone();
        let navigation_handler =
            NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let decision = navigation_panics.call(|| {
                    let mut uri = PWSTR::null();
                    let uri_result = unsafe { args.Uri(&mut uri) };
                    let uri = webview2_com::take_pwstr(uri);
                    uri_result.map(|()| {
                        weak_document
                            .upgrade()
                            .is_some_and(|doc| doc.permits_navigation(&uri))
                    })
                });
                // Only an explicit permit lets the navigation continue; a
                // failed URI read or a panic cancels it.
                let permitted = matches!(decision, Some(Ok(true)));
                let cancelled = unsafe { args.SetCancel(!permitted) };
                match decision {
                    Some(Err(error)) => cancelled.and(Err(error)),
                    _ => cancelled,
                }
            }));
        unsafe {
            self.core
                .add_NavigationStarting(&navigation_handler, &mut token)
        }
        .map_err(HostError::from_com)?;
        self.navigation = Some(token);

        // Records only that the pinned document finished loading; no
        // application code runs inside the COM callback.
        let document_loaded = Rc::clone(&self.document_loaded);
        let navigation_failed = Rc::clone(&self.navigation_failed);
        let first_completion = Cell::new(true);
        let completed_panics = panics.clone();
        let completed_handler =
            NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                completed_panics
                    .call(|| {
                        if let Some(args) = args {
                            let mut success = windows::core::BOOL::default();
                            unsafe { args.IsSuccess(&mut success) }?;
                            // Only the first completion is the pinned document's
                            // own navigation; later failures are canceled
                            // in-page navigations and do not count.
                            let first = first_completion.replace(false);
                            if success.as_bool() {
                                document_loaded.set(true);
                            } else if first {
                                navigation_failed.set(true);
                            }
                        }
                        Ok(())
                    })
                    .unwrap_or(Ok(()))
            }));
        unsafe {
            self.core
                .add_NavigationCompleted(&completed_handler, &mut token)
        }
        .map_err(HostError::from_com)?;
        self.navigation_completed = Some(token);

        let weak_document = Rc::downgrade(document);
        let message_panics = panics.clone();
        let message_handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            // The whole body, including parsing the untrusted JSON, runs
            // under the guard. The callback only enqueues into host-owned
            // state; application code runs later in session.poll.
            message_panics
                .call(|| {
                    if let (Some(args), Some(document)) = (args, weak_document.upgrade()) {
                        let mut source = PWSTR::null();
                        let mut json = PWSTR::null();
                        let source_result = unsafe { args.Source(&mut source) };
                        let source = webview2_com::take_pwstr(source);
                        source_result?;
                        let json_result = unsafe { args.WebMessageAsJson(&mut json) };
                        let json = webview2_com::take_pwstr(json);
                        json_result?;
                        if let (Ok(message), Some(messages)) = (
                            document.parse_message(json.as_bytes(), &source),
                            messages.upgrade(),
                        ) {
                            messages.borrow_mut().push_back(message);
                        }
                    }
                    Ok(())
                })
                .unwrap_or(Ok(()))
        }));
        unsafe {
            self.core
                .add_WebMessageReceived(&message_handler, &mut token)
        }
        .map_err(HostError::from_com)?;
        self.message = Some(token);

        let window_panics = panics.clone();
        let new_window_handler =
            NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                window_panics
                    .call(|| unsafe { args.SetHandled(true) })
                    // Never let a new window open: retry outside the guard.
                    .unwrap_or_else(|| unsafe { args.SetHandled(true) })
            }));
        unsafe {
            self.core
                .add_NewWindowRequested(&new_window_handler, &mut token)
        }
        .map_err(HostError::from_com)?;
        self.new_window = Some(token);
        Ok(())
    }

    /// Failed removals retain their token so a subsequent close can retry.
    pub(crate) fn close(&mut self) -> Result<(), HostError> {
        let mut first_error = None;
        for (token, remove) in [
            (
                &mut self.new_window,
                ICoreWebView2::remove_NewWindowRequested
                    as unsafe fn(&ICoreWebView2, i64) -> windows::core::Result<()>,
            ),
            (&mut self.message, ICoreWebView2::remove_WebMessageReceived),
            (
                &mut self.navigation_completed,
                ICoreWebView2::remove_NavigationCompleted,
            ),
            (
                &mut self.navigation,
                ICoreWebView2::remove_NavigationStarting,
            ),
            (
                &mut self.resource,
                ICoreWebView2::remove_WebResourceRequested,
            ),
        ] {
            if let Some(value) = *token {
                match unsafe { remove(&self.core, value) } {
                    Ok(()) => *token = None,
                    Err(error) => {
                        first_error.get_or_insert(HostError::from_com(error));
                    }
                }
            }
        }
        if self.filter && self.resource.is_none() {
            let filter = wide("*");
            match unsafe {
                self.core.RemoveWebResourceRequestedFilter(
                    PCWSTR::from_raw(filter.as_ptr()),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                )
            } {
                Ok(()) => self.filter = false,
                Err(error) => {
                    first_error.get_or_insert(HostError::from_com(error));
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(crate) fn resource_failed(&self) -> bool {
        self.resource_failed.get()
    }

    pub(crate) fn navigation_failed(&self) -> bool {
        self.navigation_failed.get()
    }

    pub(crate) fn document_loaded(&self) -> bool {
        self.document_loaded.get()
    }
}

impl Drop for CallbackTokens {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// The guard is the only path to `Complete`. It retains the deferral on
/// failed response assignment or failed completion so teardown can abort the
/// controller before the deferral is released.
struct ResourceGate<D> {
    deferral: Option<D>,
    responded: bool,
}

impl<D> ResourceGate<D> {
    fn new(deferral: D) -> Self {
        Self {
            deferral: Some(deferral),
            responded: false,
        }
    }

    fn response_installed(&mut self) {
        self.responded = true;
    }

    fn complete_if_responded<E>(
        &mut self,
        complete: impl FnOnce(&D) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.responded {
            complete(self.deferral.as_ref().expect("deferral retained"))?;
            self.deferral.take();
        }
        Ok(())
    }

    fn take_pending(&mut self) -> Option<D> {
        self.deferral.take()
    }
}

fn install_response<D, E>(
    gate: &mut ResourceGate<D>,
    primary: impl FnOnce() -> Result<(), E>,
    deny: impl FnOnce() -> Result<(), E>,
) -> Result<(), E> {
    if primary().is_ok() {
        gate.response_installed();
        return Ok(());
    }
    deny()?;
    gate.response_installed();
    Ok(())
}

fn response_decision<'a>(
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    document: Option<&'a HostDocument>,
) -> windows::core::Result<ResponseSpec<'a>> {
    match document {
        Some(document) => resource_decision(args, document),
        None => Ok(ResponseSpec::forbidden()),
    }
}

fn create_response(
    environment: &ICoreWebView2Environment,
    response: ResponseSpec<'_>,
) -> windows::core::Result<ICoreWebView2WebResourceResponse> {
    // A document streams from its shared buffer (no per-request copy); small
    // fixed bodies (denials, probe assets) are copied into an HGLOBAL.
    if let Some(shared) = response.shared_body.filter(|_| !copy_for_measurement()) {
        let stream = crate::stream::SharedStream::create(std::sync::Arc::clone(shared));
        return create_response_from(environment, &stream, response);
    }
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }?;
    if !response.body.is_empty() {
        let mut written = 0;
        unsafe {
            stream.Write(
                response.body.as_ptr().cast::<c_void>(),
                response.body.len() as u32,
                Some(&mut written),
            )
        }
        .ok()?;
        if written != response.body.len() as u32 {
            return Err(windows::core::HRESULT(0x8000_4005u32 as i32).into());
        }
    }
    unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }?;
    create_response_from(environment, &stream, response)
}

/// Native measurements compare streaming with the old per-request copy.
#[cfg(test)]
pub(crate) static COPY_FOR_MEASUREMENT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn copy_for_measurement() -> bool {
    #[cfg(test)]
    {
        COPY_FOR_MEASUREMENT.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(test))]
    {
        false
    }
}

fn create_response_from(
    environment: &ICoreWebView2Environment,
    stream: &windows::Win32::System::Com::IStream,
    response: ResponseSpec<'_>,
) -> windows::core::Result<ICoreWebView2WebResourceResponse> {
    let reason = wide(response.reason);
    let headers = wide(&format!(
        "Content-Type: {}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nContent-Security-Policy: {}\r\n",
        response.content_type, response.content_security_policy,
    ));
    unsafe {
        environment.CreateWebResourceResponse(
            stream,
            response.status,
            PCWSTR::from_raw(reason.as_ptr()),
            PCWSTR::from_raw(headers.as_ptr()),
        )
    }
}

fn resource_decision<'a>(
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    document: &'a HostDocument,
) -> windows::core::Result<ResponseSpec<'a>> {
    let request = unsafe { args.Request() }?;
    let mut method = PWSTR::null();
    let method_result = unsafe { request.Method(&mut method) };
    let method = webview2_com::take_pwstr(method);
    method_result?;
    let mut uri = PWSTR::null();
    let uri_result = unsafe { request.Uri(&mut uri) };
    let uri = webview2_com::take_pwstr(uri);
    uri_result?;
    let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
    unsafe { args.ResourceContext(&mut context) }?;
    let kind = if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
        ResourceKind::Document
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE {
        ResourceKind::Image
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT {
        ResourceKind::Script
    } else if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET {
        ResourceKind::Stylesheet
    } else {
        ResourceKind::Other
    };
    Ok(document.resolve(&BrokerRequest {
        method: &method,
        uri: &uri,
        controller_tab_id: document.tab_id(),
        kind,
    }))
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod resource_gate_tests {
    use std::cell::Cell;

    use super::{ResourceGate, install_response};

    #[test]
    fn a_failed_primary_and_deny_assignment_never_completes_the_deferral() {
        let completed = Cell::new(false);
        let deny_attempted = Cell::new(false);
        let mut gate = ResourceGate::new(7_u8);
        assert!(
            install_response(
                &mut gate,
                || Err::<(), _>(()),
                || {
                    deny_attempted.set(true);
                    Err::<(), _>(())
                },
            )
            .is_err()
        );
        assert!(deny_attempted.get());
        gate.complete_if_responded(|_| {
            completed.set(true);
            Ok::<_, ()>(())
        })
        .unwrap();
        assert!(!completed.get());
        assert_eq!(gate.take_pending(), Some(7));
    }

    #[test]
    fn a_successfully_installed_deny_response_can_complete_the_deferral() {
        let completed = Cell::new(false);
        let mut gate = ResourceGate::new(9_u8);
        install_response(&mut gate, || Err::<(), _>(()), || Ok(())).unwrap();
        gate.complete_if_responded(|_| {
            completed.set(true);
            Ok::<_, ()>(())
        })
        .unwrap();
        assert!(completed.get());
        assert_eq!(gate.take_pending(), None);
    }

    #[test]
    fn a_failed_complete_retains_the_deferral_for_controller_abort() {
        let mut gate = ResourceGate::new(11_u8);
        gate.response_installed();
        assert!(gate.complete_if_responded(|_| Err::<(), _>(())).is_err());
        assert_eq!(gate.take_pending(), Some(11));
    }
}
