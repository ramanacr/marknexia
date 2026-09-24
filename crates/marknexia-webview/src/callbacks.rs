//! WebView2 event sinks. Every callback uses immutable policy and weak app state.

use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::{Rc, Weak},
};

use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
        ICoreWebView2, ICoreWebView2Environment, ICoreWebView2WebResourceRequestedEventArgs,
    },
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler,
};
use windows::{
    Win32::{
        Foundation::HGLOBAL,
        System::Com::{STREAM_SEEK_SET, StructuredStorage::CreateStreamOnHGlobal},
    },
    core::{PCWSTR, PWSTR},
};

use crate::{
    broker::{BrokerRequest, ResourceKind},
    host::HostError,
    policy::{HostDocument, ResponseSpec},
    protocol::PageToHost,
};

pub(crate) struct CallbackTokens {
    core: ICoreWebView2,
    resource: Option<i64>,
    navigation: Option<i64>,
    message: Option<i64>,
    new_window: Option<i64>,
    filter: bool,
}

impl CallbackTokens {
    pub(crate) fn new(core: &ICoreWebView2) -> Self {
        Self {
            core: core.clone(),
            resource: None,
            navigation: None,
            message: None,
            new_window: None,
            filter: false,
        }
    }

    pub(crate) fn register(
        &mut self,
        environment: &ICoreWebView2Environment,
        document: &Rc<HostDocument>,
        observer: Weak<dyn Fn(PageToHost)>,
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
        let resource_handler =
            WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                // Always complete a deferral, including the deny/error path. The
                // policy and response stream are bounded before WebView receives it.
                let deferral = unsafe { args.GetDeferral() }?;
                let result = respond_to_resource(
                    &response_environment,
                    &args,
                    weak_document.upgrade().as_deref(),
                );
                let completion = unsafe { deferral.Complete() };
                result.and(completion)
            }));
        let mut token = 0;
        unsafe {
            self.core
                .add_WebResourceRequested(&resource_handler, &mut token)
        }
        .map_err(HostError::from_com)?;
        self.resource = Some(token);

        let weak_document = Rc::downgrade(document);
        let navigation_handler =
            NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut uri = PWSTR::null();
                    unsafe { args.Uri(&mut uri) }?;
                    let uri = webview2_com::take_pwstr(uri);
                    let permitted = weak_document
                        .upgrade()
                        .is_some_and(|doc| doc.permits_navigation(&uri));
                    unsafe { args.SetCancel(!permitted) }?;
                }
                Ok(())
            }));
        unsafe {
            self.core
                .add_NavigationStarting(&navigation_handler, &mut token)
        }
        .map_err(HostError::from_com)?;
        self.navigation = Some(token);

        let weak_document = Rc::downgrade(document);
        let message_handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            if let (Some(args), Some(document)) = (args, weak_document.upgrade()) {
                let mut source = PWSTR::null();
                let mut json = PWSTR::null();
                let source_result = unsafe { args.Source(&mut source) };
                let source = webview2_com::take_pwstr(source);
                source_result?;
                let json_result = unsafe { args.WebMessageAsJson(&mut json) };
                let json = webview2_com::take_pwstr(json);
                json_result?;
                if let (Ok(message), Some(observer)) = (
                    document.parse_message(json.as_bytes(), &source),
                    observer.upgrade(),
                ) {
                    // Application callbacks are untrusted from the COM ABI's
                    // perspective. A panic must never unwind into WebView2.
                    let _ = catch_unwind(AssertUnwindSafe(|| observer(message)));
                }
            }
            Ok(())
        }));
        unsafe {
            self.core
                .add_WebMessageReceived(&message_handler, &mut token)
        }
        .map_err(HostError::from_com)?;
        self.message = Some(token);

        let new_window_handler =
            NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    unsafe { args.SetHandled(true) }?;
                }
                Ok(())
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
}

impl Drop for CallbackTokens {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn respond_to_resource(
    environment: &ICoreWebView2Environment,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    document: Option<&HostDocument>,
) -> windows::core::Result<()> {
    let response = document
        .and_then(|document| resource_decision(args, document).ok())
        .unwrap_or_else(ResponseSpec::forbidden);
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }?;
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
        return windows::core::HRESULT(0x8000_4005u32 as i32).ok();
    }
    unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }?;
    let reason = wide(response.reason);
    let headers = wide(&format!(
        "Content-Type: {}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; img-src 'self'; style-src 'self'; script-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n",
        response.content_type,
    ));
    let response = unsafe {
        environment.CreateWebResourceResponse(
            &stream,
            response.status,
            PCWSTR::from_raw(reason.as_ptr()),
            PCWSTR::from_raw(headers.as_ptr()),
        )
    }?;
    unsafe { args.SetResponse(&response) }
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
        controller_tab_id: document.tab_id,
        kind,
    }))
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
