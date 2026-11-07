//! STA-owned controller fleet. COM callbacks enqueue events; the host pumps them.

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    rc::{Rc, Weak},
};

use windows::Win32::Foundation::HWND;

use crate::{
    environment::{
        BrowserExit, BrowserExitSubscription, EnvironmentError, StaApartment, WebViewEnvironment,
    },
    host::{HostColor, HostError, ProcessFailure, ViewportBounds, WebViewHost},
    panic_flag::PanicFlag,
    policy::{DocumentError, HostDocument},
    protocol::PageToHost,
    recovery::{RecoveryAction, RecoveryCoordinator},
};

enum SessionEvent {
    EnvironmentCreated(u64, Result<WebViewEnvironment, EnvironmentError>),
    /// Boxed: a host is much larger than the other events.
    HostCreated(u64, u64, u64, Box<Result<WebViewHost, HostError>>),
    ProcessFailure(u64, u64, u64, ProcessFailure),
    PageMessage(u64, u64, u64, PageToHost),
    BrowserExited(u64, BrowserExit),
}

#[derive(Debug)]
pub enum SessionError {
    Closed,
    AlreadyStarted,
    Recovering,
    DuplicateTab,
    MissingTab,
    Document(DocumentError),
    Environment(EnvironmentError),
    Host(HostError),
    ResourceBoundaryFailed,
    /// A panic was caught inside a WebView2 callback, so a failure or page
    /// notification may have been lost. Reported once per caught panic.
    CallbackPanicked,
}

/// Why one tab's document cannot be shown. Reported per tab through
/// [`WebViewSession::drain_tab_failures`]; other tabs are unaffected, and a
/// failed tab is not retried until the environment is recreated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabFailure {
    /// The controller could not be created.
    ControllerStartup(HostError),
    /// The controller was created but could not be set up for the document.
    ControllerSetup(HostError),
    /// The document's first navigation completed without success.
    NavigationFailed,
}

impl SessionError {
    /// The WebView2 environment could not start: no document can be shown
    /// in this session.
    #[must_use]
    pub fn is_environment_failure(&self) -> bool {
        matches!(self, Self::Environment(_))
    }
}

impl From<EnvironmentError> for SessionError {
    fn from(error: EnvironmentError) -> Self {
        Self::Environment(error)
    }
}

impl From<HostError> for SessionError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

/// Own this value on the STA that owns `parent`. Call `poll` from the shell's
/// message loop after dispatch; callbacks only enqueue and never borrow this
/// session mutably across a COM call. `close` precedes destruction of `parent`.
pub struct WebViewSession {
    apartment: Rc<StaApartment>,
    parent: HWND,
    user_data_folder: PathBuf,
    page_observer: Weak<dyn Fn(PageToHost)>,
    events: Rc<RefCell<VecDeque<SessionEvent>>>,
    documents: BTreeMap<u64, HostDocument>,
    hosts: BTreeMap<u64, WebViewHost>,
    orphaned_hosts: BTreeMap<u64, Vec<WebViewHost>>,
    /// Per tab, how many orphans whose close keeps failing were reported.
    orphan_failures_reported: BTreeMap<u64, usize>,
    process_observers: BTreeMap<u64, Rc<dyn Fn(ProcessFailure)>>,
    browser_observer: Option<Rc<dyn Fn(BrowserExit)>>,
    browser_subscription: Option<BrowserExitSubscription>,
    environment: Option<WebViewEnvironment>,
    recovery: RecoveryCoordinator,
    pending_hosts: BTreeSet<u64>,
    controller_generations: BTreeMap<u64, u64>,
    next_controller_generation: u64,
    recreating_tabs: BTreeSet<u64>,
    visibility_pending: bool,
    security_failed: bool,
    generation: u64,
    creating_environment: bool,
    teardown_pending: bool,
    environment_recreate_pending: bool,
    active_tab_restore_pending: Option<u64>,
    browser_recovering: bool,
    recovery_generation: Option<u64>,
    closed: bool,
    viewport: ViewportBounds,
    background: HostColor,
    /// Shared by every controller, browser-exit subscription and the page
    /// observer guard: a panic caught in any of them is reported by the next
    /// `poll`, even after that controller or subscription is gone.
    callback_panics: PanicFlag,
    /// Tabs whose controller failed; skipped by the retry loop until the
    /// environment is recreated or the document is removed.
    failed_tabs: BTreeSet<u64>,
    tab_failures: Vec<(u64, TabFailure)>,
    /// Controller completions lost to a panic, as (generation, controller
    /// generation, tab). A `Cell` so recording can never fail; `poll` turns
    /// each into a failed `HostCreated`.
    lost_hosts: Rc<Cell<Vec<(u64, u64, u64)>>>,
}

impl WebViewSession {
    pub fn new(
        apartment: Rc<StaApartment>,
        parent: HWND,
        user_data_folder: PathBuf,
        page_observer: Weak<dyn Fn(PageToHost)>,
    ) -> Self {
        Self {
            apartment,
            parent,
            user_data_folder,
            page_observer,
            events: Rc::new(RefCell::new(VecDeque::new())),
            documents: BTreeMap::new(),
            hosts: BTreeMap::new(),
            orphaned_hosts: BTreeMap::new(),
            orphan_failures_reported: BTreeMap::new(),
            process_observers: BTreeMap::new(),
            browser_observer: None,
            browser_subscription: None,
            environment: None,
            recovery: RecoveryCoordinator::new(None),
            pending_hosts: BTreeSet::new(),
            controller_generations: BTreeMap::new(),
            next_controller_generation: 0,
            recreating_tabs: BTreeSet::new(),
            visibility_pending: false,
            security_failed: false,
            generation: 0,
            creating_environment: false,
            teardown_pending: false,
            environment_recreate_pending: false,
            active_tab_restore_pending: None,
            browser_recovering: false,
            recovery_generation: None,
            closed: false,
            viewport: ViewportBounds {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            background: HostColor {
                alpha: 255,
                red: 255,
                green: 255,
                blue: 255,
            },
            callback_panics: PanicFlag::default(),
            failed_tabs: BTreeSet::new(),
            tab_failures: Vec::new(),
            lost_hosts: Rc::new(Cell::new(Vec::new())),
        }
    }

    /// Tab-scoped failures since the last drain, in order.
    pub fn drain_tab_failures(&mut self) -> Vec<(u64, TabFailure)> {
        std::mem::take(&mut self.tab_failures)
    }

    /// Records a tab-scoped failure once; the tab is no longer retried. Any
    /// live controller for it is closed (or orphaned for a later close), so
    /// a failed tab never keeps a hidden or half-loaded controller.
    fn fail_tab(&mut self, tab_id: u64, failure: TabFailure) {
        self.pending_hosts.remove(&tab_id);
        self.recreating_tabs.remove(&tab_id);
        self.controller_generations.remove(&tab_id);
        self.process_observers.remove(&tab_id);
        if let Some(mut host) = self.hosts.remove(&tab_id) {
            let _ = host.set_visible(false);
            if host.close().is_err() {
                self.orphaned_hosts.entry(tab_id).or_default().push(host);
            }
        }
        if self.failed_tabs.insert(tab_id) {
            self.tab_failures.push((tab_id, failure));
        }
    }

    pub fn add_document(&mut self, document: HostDocument) -> Result<(), SessionError> {
        if self.closed {
            return Err(SessionError::Closed);
        }
        if self.security_failed {
            return Err(SessionError::ResourceBoundaryFailed);
        }
        document.validate().map_err(SessionError::Document)?;
        let tab_id = document.tab_id();
        if self.documents.contains_key(&tab_id) {
            return Err(SessionError::DuplicateTab);
        }
        self.documents.insert(tab_id, document);
        if self.environment.is_some() && !self.browser_recovering {
            self.begin_host(tab_id)?;
        }
        Ok(())
    }

    pub fn select_tab(&mut self, tab_id: u64) -> Result<(), SessionError> {
        if self.security_failed {
            return Err(SessionError::ResourceBoundaryFailed);
        }
        if self.browser_recovering {
            return Err(SessionError::Recovering);
        }
        if !self.documents.contains_key(&tab_id) {
            return Err(SessionError::MissingTab);
        }
        self.recovery.set_active_tab(Some(tab_id));
        self.visibility_pending = true;
        self.apply_selected_visibility()?;
        Ok(())
    }

    /// Hide every controller: the shell's active tab has no document yet (a
    /// render is pending). A later `select_tab` shows a controller again.
    pub fn clear_selection(&mut self) -> Result<(), SessionError> {
        if self.security_failed {
            return Err(SessionError::ResourceBoundaryFailed);
        }
        if self.browser_recovering {
            return Err(SessionError::Recovering);
        }
        self.recovery.set_active_tab(None);
        self.visibility_pending = true;
        self.apply_selected_visibility()?;
        Ok(())
    }

    #[must_use]
    pub fn has_document(&self, tab_id: u64) -> bool {
        self.documents.contains_key(&tab_id)
    }

    /// Apply shell-owned client bounds to every live controller and remember
    /// them for controllers that complete asynchronously after a DPI/layout
    /// change.
    pub fn set_viewport(&mut self, bounds: ViewportBounds) -> Result<(), SessionError> {
        if self.closed {
            return Err(SessionError::Closed);
        }
        self.viewport = bounds;
        for host in self.hosts.values() {
            host.set_bounds(bounds)?;
        }
        Ok(())
    }

    pub fn set_background(&mut self, color: HostColor) -> Result<(), SessionError> {
        if self.closed {
            return Err(SessionError::Closed);
        }
        self.background = color;
        for host in self.hosts.values() {
            host.set_default_background(color)?;
        }
        Ok(())
    }

    /// Remove a tab's immutable document and close its controller before the
    /// shell discards the corresponding portable tab identity.
    pub fn remove_document(&mut self, tab_id: u64) -> Result<(), SessionError> {
        if self.closed {
            return Err(SessionError::Closed);
        }
        if self.documents.remove(&tab_id).is_none() {
            return Err(SessionError::MissingTab);
        }
        self.pending_hosts.remove(&tab_id);
        self.recreating_tabs.remove(&tab_id);
        self.controller_generations.remove(&tab_id);
        self.process_observers.remove(&tab_id);
        self.failed_tabs.remove(&tab_id);
        self.tab_failures.retain(|(failed, _)| *failed != tab_id);
        if let Some(mut host) = self.hosts.remove(&tab_id)
            && let Err(error) = host.close()
        {
            self.orphaned_hosts.entry(tab_id).or_default().push(host);
            return Err(SessionError::Host(error));
        }
        Ok(())
    }

    /// Shows `document` in its tab instead of the tab's current document
    /// (a followed link, back or forward, a reload). A live controller is
    /// reused: it navigates to the new document in place, so the old page
    /// stays painted until the new one commits and no controller is
    /// created. Page messages of the old document still queued here are
    /// dropped. Without a usable controller (none yet, failed, recovering)
    /// the document is replaced as `remove_document` plus `add_document`.
    pub fn replace_document(&mut self, document: HostDocument) -> Result<(), SessionError> {
        if self.closed {
            return Err(SessionError::Closed);
        }
        if self.security_failed {
            return Err(SessionError::ResourceBoundaryFailed);
        }
        document.validate().map_err(SessionError::Document)?;
        let tab_id = document.tab_id();
        if !self.documents.contains_key(&tab_id) {
            return self.add_document(document);
        }
        let reusable = !self.browser_recovering
            && !self.failed_tabs.contains(&tab_id)
            && !self.recreating_tabs.contains(&tab_id);
        if reusable
            && let Some(host) = self.hosts.get_mut(&tab_id)
            && host.rebind_document(document.clone()).is_ok()
        {
            self.documents.insert(tab_id, document);
            self.events.borrow_mut().retain(
                |event| !matches!(event, SessionEvent::PageMessage(_, _, tab, _) if *tab == tab_id),
            );
            return Ok(());
        }
        // No controller, or one that may be half-switched: close it and
        // start over.
        let _ = self.remove_document(tab_id);
        self.add_document(document)
    }

    /// User-initiated navigations that a tab's page started without the
    /// link adapter (and that were cancelled), as (tab, epoch, document
    /// URI, requested URI).
    pub fn drain_navigation_requests(&self) -> Vec<(u64, u64, String, String)> {
        let mut requests = Vec::new();
        for (tab_id, host) in &self.hosts {
            let Some(document) = self.documents.get(tab_id) else {
                continue;
            };
            for uri in host.drain_navigation_requests() {
                requests.push((
                    *tab_id,
                    document.document_epoch(),
                    document.document_uri(),
                    uri,
                ));
            }
        }
        requests
    }

    pub fn start(&mut self) -> Result<(), SessionError> {
        if self.closed {
            return Err(SessionError::Closed);
        }
        if self.security_failed {
            return Err(SessionError::ResourceBoundaryFailed);
        }
        if self.environment.is_some() || self.creating_environment || self.browser_recovering {
            return Err(SessionError::AlreadyStarted);
        }
        self.begin_environment()
    }

    fn begin_environment(&mut self) -> Result<(), SessionError> {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let events = Rc::clone(&self.events);
        self.creating_environment = true;
        let result = WebViewEnvironment::create_async(
            Rc::clone(&self.apartment),
            &self.user_data_folder,
            &self.callback_panics,
            Box::new(move |result| {
                events
                    .borrow_mut()
                    .push_back(SessionEvent::EnvironmentCreated(generation, result));
            }),
        );
        if let Err(error) = result {
            self.creating_environment = false;
            return Err(SessionError::Environment(error));
        }
        Ok(())
    }

    fn begin_host(&mut self, tab_id: u64) -> Result<(), SessionError> {
        // A failed tab is retried only after `failed_tabs` is cleared (a
        // recreated environment), never by an ordinary begin_host path.
        if self.failed_tabs.contains(&tab_id)
            || self.hosts.contains_key(&tab_id)
            || !self.pending_hosts.insert(tab_id)
        {
            return Ok(());
        }
        let Some(environment) = self.environment.as_ref() else {
            self.pending_hosts.remove(&tab_id);
            return Err(SessionError::Environment(
                EnvironmentError::MissingEnvironment,
            ));
        };
        let events = Rc::clone(&self.events);
        let generation = self.generation;
        self.next_controller_generation = self.next_controller_generation.wrapping_add(1);
        let controller_generation = self.next_controller_generation;
        self.controller_generations
            .insert(tab_id, controller_generation);
        let lost_hosts = Rc::clone(&self.lost_hosts);
        let result = environment.create_host_async(
            self.parent,
            &self.callback_panics,
            Box::new(move |result| {
                events.borrow_mut().push_back(SessionEvent::HostCreated(
                    generation,
                    controller_generation,
                    tab_id,
                    Box::new(result),
                ));
            }),
            // The completion panicked: still queue a failure, so the tab
            // never stays pending.
            Box::new(move || {
                // Cannot fail: poll turns this into a failed HostCreated.
                let mut lost = lost_hosts.take();
                lost.push((generation, controller_generation, tab_id));
                lost_hosts.set(lost);
            }),
        );
        if let Err(error) = result {
            // Tab-scoped: later tabs still start in the same pass.
            self.controller_generations.remove(&tab_id);
            self.fail_tab(tab_id, TabFailure::ControllerStartup(error));
        }
        Ok(())
    }

    /// Drain after Win32 dispatch, never inside a WebView2 callback.
    pub fn poll(&mut self) -> Result<(), SessionError> {
        if self.closed {
            return Err(SessionError::Closed);
        }
        self.abort_failed_resources()?;
        if self.security_failed {
            return Err(SessionError::ResourceBoundaryFailed);
        }
        for (generation, controller_generation, tab_id) in self.lost_hosts.take() {
            self.events
                .borrow_mut()
                .push_back(SessionEvent::HostCreated(
                    generation,
                    controller_generation,
                    tab_id,
                    Box::new(Err(HostError::CallbackPanicked)),
                ));
        }
        loop {
            self.enqueue_page_messages();
            let next = { self.events.borrow_mut().pop_front() };
            let Some(event) = next else {
                break;
            };
            self.apply_event(event)?;
            self.abort_failed_resources()?;
            if self.security_failed {
                return Err(SessionError::ResourceBoundaryFailed);
            }
        }
        if self.teardown_pending {
            self.finish_teardown()?;
        }
        if self.environment_recreate_pending {
            self.recreate_environment()?;
        }
        let mut orphan_error = None;
        if self.environment.is_some()
            && !self.teardown_pending
            && !self.environment_recreate_pending
        {
            if !self.browser_recovering {
                self.retry_renderer_recreation()?;
            }
            // A stuck orphan does not stop new controllers from starting.
            orphan_error = self.close_orphaned_hosts();
            for tab_id in self.documents.keys().copied().collect::<Vec<_>>() {
                if !self.hosts.contains_key(&tab_id)
                    && !self.orphaned_hosts.contains_key(&tab_id)
                    && !self.recreating_tabs.contains(&tab_id)
                    && !self.failed_tabs.contains(&tab_id)
                {
                    self.begin_host(tab_id)?;
                }
            }
            // A document whose first navigation failed is a tab failure.
            let failed_navigations: Vec<u64> = self
                .hosts
                .iter()
                .filter(|(tab_id, host)| {
                    host.navigation_failed() && !self.failed_tabs.contains(tab_id)
                })
                .map(|(tab_id, _)| *tab_id)
                .collect();
            for tab_id in failed_navigations {
                self.fail_tab(tab_id, TabFailure::NavigationFailed);
            }
        }
        self.finish_environment_restore()?;
        if let Some(tab_id) = self.active_tab_restore_pending {
            self.restore_active_tab(tab_id)?;
        }
        if self.visibility_pending {
            self.apply_selected_visibility()?;
        }
        if let Some(error) = orphan_error {
            return Err(error);
        }
        self.take_callback_panics()
    }

    /// Surfaces, once, a panic caught in any callback since the last poll.
    fn take_callback_panics(&self) -> Result<(), SessionError> {
        if self.callback_panics.take() {
            Err(SessionError::CallbackPanicked)
        } else {
            Ok(())
        }
    }

    fn apply_event(&mut self, event: SessionEvent) -> Result<(), SessionError> {
        match event {
            SessionEvent::EnvironmentCreated(generation, result) => {
                if generation != self.generation {
                    return Ok(());
                }
                self.creating_environment = false;
                let environment = result?;
                let events = Rc::clone(&self.events);
                let observer: Rc<dyn Fn(BrowserExit)> = Rc::new(move |exit| {
                    events
                        .borrow_mut()
                        .push_back(SessionEvent::BrowserExited(generation, exit));
                });
                let subscription = environment
                    .observe_browser_exit(Rc::downgrade(&observer), &self.callback_panics)?;
                self.browser_observer = Some(observer);
                self.browser_subscription = Some(subscription);
                self.environment = Some(environment);
                // A new environment is the explicit retry for failed tabs.
                self.failed_tabs.clear();
                for tab_id in self.documents.keys().copied().collect::<Vec<_>>() {
                    self.begin_host(tab_id)?;
                }
                self.finish_environment_restore()?;
            }
            SessionEvent::HostCreated(generation, controller_generation, tab_id, result) => {
                let result = *result;
                if generation != self.generation
                    || self.controller_generations.get(&tab_id) != Some(&controller_generation)
                    || (self.browser_recovering && self.recovery_generation != Some(generation))
                    || self.closed
                {
                    if let Ok(mut host) = result
                        && host.close().is_err()
                    {
                        self.orphaned_hosts.entry(tab_id).or_default().push(host);
                    }
                    return Ok(());
                }
                self.pending_hosts.remove(&tab_id);
                let mut host = match result {
                    Ok(host) => host,
                    Err(error) => {
                        self.controller_generations.remove(&tab_id);
                        self.fail_tab(tab_id, TabFailure::ControllerStartup(error));
                        return Ok(());
                    }
                };
                // Before any callback is registered on this controller.
                host.share_callback_panics(&self.callback_panics);
                if let Err(error) = host
                    .set_bounds(self.viewport)
                    .and_then(|()| host.set_default_background(self.background))
                {
                    self.controller_generations.remove(&tab_id);
                    if host.close().is_err() {
                        self.orphaned_hosts.entry(tab_id).or_default().push(host);
                    }
                    self.fail_tab(tab_id, TabFailure::ControllerSetup(error));
                    return Ok(());
                }
                let Some(document) = self.documents.get(&tab_id).cloned() else {
                    if host.close().is_err() {
                        self.orphaned_hosts.entry(tab_id).or_default().push(host);
                    }
                    self.controller_generations.remove(&tab_id);
                    return Ok(());
                };
                let events = Rc::clone(&self.events);
                let observer: Rc<dyn Fn(ProcessFailure)> = Rc::new(move |failure| {
                    events.borrow_mut().push_back(SessionEvent::ProcessFailure(
                        generation,
                        controller_generation,
                        tab_id,
                        failure,
                    ));
                });
                if let Err(error) = host
                    .observe_process_failures(Rc::downgrade(&observer))
                    .and_then(|()| host.intercept_shell_accelerators())
                    .and_then(|()| host.bind_document(document))
                {
                    self.controller_generations.remove(&tab_id);
                    if host.close().is_err() {
                        self.orphaned_hosts.entry(tab_id).or_default().push(host);
                    }
                    self.fail_tab(tab_id, TabFailure::ControllerSetup(error));
                    return Ok(());
                }
                self.process_observers.insert(tab_id, observer);
                self.hosts.insert(tab_id, host);
                self.visibility_pending = true;
                self.apply_selected_visibility()?;
                self.recovery.renderer_restored(tab_id);
                self.finish_environment_restore()?;
            }
            SessionEvent::ProcessFailure(
                generation,
                controller_generation,
                tab_id,
                ProcessFailure::BrowserExited,
            ) if generation == self.generation
                && self.controller_generations.get(&tab_id) == Some(&controller_generation) =>
            {
                let action = self.recovery.record_browser_exit();
                self.apply_recovery_action(action)?;
                self.process_observers.remove(&tab_id);
            }
            SessionEvent::ProcessFailure(
                generation,
                controller_generation,
                tab_id,
                ProcessFailure::RendererExited | ProcessFailure::RendererUnresponsive,
            ) if generation == self.generation
                && self.controller_generations.get(&tab_id) == Some(&controller_generation) =>
            {
                let action = self.recovery.record_renderer_failure(tab_id);
                self.apply_recovery_action(action)?;
            }
            SessionEvent::ProcessFailure(_, _, _, _) => {}
            SessionEvent::PageMessage(generation, controller_generation, tab_id, message)
                if generation == self.generation
                    && self.controller_generations.get(&tab_id) == Some(&controller_generation)
                    && self.hosts.contains_key(&tab_id) =>
            {
                if let Some(observer) = self.page_observer.upgrade() {
                    // Application code: a panic is recorded, not unwound
                    // into the shell's message loop.
                    self.callback_panics.guard(|| observer(message));
                }
            }
            SessionEvent::PageMessage(_, _, _, _) => {}
            SessionEvent::BrowserExited(generation, _exit) if generation == self.generation => {
                let action = self.recovery.browser_process_exited();
                self.apply_recovery_action(action)?;
            }
            SessionEvent::BrowserExited(_, _) => {}
        }
        Ok(())
    }

    fn apply_recovery_action(&mut self, action: RecoveryAction) -> Result<(), SessionError> {
        match action {
            RecoveryAction::None => {}
            RecoveryAction::RecreateController { tab_id } => {
                self.controller_generations.remove(&tab_id);
                self.recreating_tabs.insert(tab_id);
                self.retry_renderer_recreation()?;
            }
            RecoveryAction::CloseAllControllers => {
                self.browser_recovering = true;
                self.teardown_pending = true;
                self.recreating_tabs.clear();
                self.controller_generations.clear();
                self.finish_teardown()?;
            }
            RecoveryAction::RecreateEnvironment => {
                self.environment_recreate_pending = true;
                self.recreate_environment()?;
            }
            RecoveryAction::RestoreActiveTab { tab_id } => {
                self.active_tab_restore_pending = Some(tab_id);
                self.restore_active_tab(tab_id)?;
            }
        }
        Ok(())
    }

    fn finish_teardown(&mut self) -> Result<(), SessionError> {
        let mut first_error = None;
        for tab_id in self.hosts.keys().copied().collect::<Vec<_>>() {
            if let Some(host) = self.hosts.get_mut(&tab_id) {
                match host.close() {
                    Ok(()) => {
                        self.hosts.remove(&tab_id);
                        self.process_observers.remove(&tab_id);
                    }
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
        }
        if let Some(error) = first_error {
            return Err(SessionError::Host(error));
        }
        if let Some(error) = self.close_orphaned_hosts() {
            return Err(error);
        }
        self.teardown_pending = false;
        let action = self.recovery.controllers_closed();
        self.apply_recovery_action(action)
    }

    fn retry_renderer_recreation(&mut self) -> Result<(), SessionError> {
        for tab_id in self.recreating_tabs.iter().copied().collect::<Vec<_>>() {
            if self.failed_tabs.contains(&tab_id) {
                self.recreating_tabs.remove(&tab_id);
                continue;
            }
            if let Some(host) = self.hosts.get_mut(&tab_id) {
                host.close()?;
            }
            self.hosts.remove(&tab_id);
            self.process_observers.remove(&tab_id);
            if self.orphaned_hosts.contains_key(&tab_id) {
                continue;
            }
            self.begin_host(tab_id)?;
            self.recreating_tabs.remove(&tab_id);
        }
        Ok(())
    }

    /// Retries closing every orphaned controller. One that keeps failing
    /// stays orphaned without stopping the others or the caller; each newly
    /// failing orphan is reported once (the first such error is returned).
    fn close_orphaned_hosts(&mut self) -> Option<SessionError> {
        let mut first_new = None;
        for tab_id in self.orphaned_hosts.keys().copied().collect::<Vec<_>>() {
            let Some(hosts) = self.orphaned_hosts.get_mut(&tab_id) else {
                continue;
            };
            let mut errors = Vec::new();
            hosts.retain_mut(|host| match host.close() {
                Ok(()) => false,
                Err(error) => {
                    errors.push(error);
                    true
                }
            });
            if hosts.is_empty() {
                self.orphaned_hosts.remove(&tab_id);
                self.orphan_failures_reported.remove(&tab_id);
                continue;
            }
            let reported = self.orphan_failures_reported.entry(tab_id).or_default();
            if errors.len() > *reported {
                first_new.get_or_insert(SessionError::Host(errors[*reported]));
            }
            *reported = errors.len();
        }
        first_new
    }

    fn recreate_environment(&mut self) -> Result<(), SessionError> {
        if let Some(subscription) = self.browser_subscription.as_mut() {
            subscription.close()?;
        }
        self.browser_subscription.take();
        self.browser_observer.take();
        self.environment.take();
        self.pending_hosts.clear();
        self.controller_generations.clear();
        self.begin_environment()?;
        self.recovery_generation = Some(self.generation);
        self.environment_recreate_pending = false;
        Ok(())
    }

    fn restore_active_tab(&mut self, tab_id: u64) -> Result<(), SessionError> {
        self.visibility_pending = true;
        self.apply_selected_visibility()?;
        self.recovery.active_tab_restored(tab_id);
        self.active_tab_restore_pending = None;
        self.browser_recovering = false;
        self.recovery_generation = None;
        Ok(())
    }

    fn finish_environment_restore(&mut self) -> Result<(), SessionError> {
        if self.browser_recovering
            && !self.creating_environment
            && self.pending_hosts.is_empty()
            // Failed tabs count as settled; they are not retried.
            && self
                .documents
                .keys()
                .all(|tab_id| self.hosts.contains_key(tab_id) || self.failed_tabs.contains(tab_id))
        {
            let action = self.recovery.environment_restored();
            if action == RecoveryAction::None && self.recovery.is_healthy() {
                self.browser_recovering = false;
                self.recovery_generation = None;
            }
            self.apply_recovery_action(action)?;
        }
        Ok(())
    }

    fn apply_selected_visibility(&mut self) -> Result<(), SessionError> {
        let active = self.recovery.active_tab_id();
        for (tab_id, host) in &self.hosts {
            host.set_visible(Some(*tab_id) == active)?;
        }
        self.visibility_pending = false;
        Ok(())
    }

    fn enqueue_page_messages(&self) {
        let mut queued = Vec::new();
        for (tab_id, host) in &self.hosts {
            if let Some(controller_generation) = self.controller_generations.get(tab_id) {
                queued.extend(host.drain_page_messages().into_iter().map(|message| {
                    SessionEvent::PageMessage(
                        self.generation,
                        *controller_generation,
                        *tab_id,
                        message,
                    )
                }));
            }
        }
        self.events.borrow_mut().extend(queued);
    }

    fn abort_failed_resources(&mut self) -> Result<(), SessionError> {
        if !self.security_failed
            && !self
                .hosts
                .values()
                .any(WebViewHost::resource_boundary_failed)
        {
            return Ok(());
        }
        self.security_failed = true;
        self.controller_generations.clear();
        // Close the entire controller fleet. No new page message or resource
        // may be accepted after one controller loses its security boundary.
        let mut first_error = None;
        for tab_id in self.hosts.keys().copied().collect::<Vec<_>>() {
            if let Some(host) = self.hosts.get_mut(&tab_id)
                && let Err(error) = host.abort_security_boundary()
            {
                first_error.get_or_insert(error);
                continue;
            }
            self.hosts.remove(&tab_id);
            self.process_observers.remove(&tab_id);
        }
        if let Some(error) = first_error {
            return Err(SessionError::Host(error));
        }
        Ok(())
    }

    /// Shell accelerators pressed inside any live document since the last
    /// drain, in arrival order per host.
    pub fn drain_accelerators(&self) -> Vec<crate::host::ShellAccelerator> {
        self.hosts
            .values()
            .flat_map(WebViewHost::drain_accelerators)
            .collect()
    }

    pub fn host(&self, tab_id: u64) -> Option<&WebViewHost> {
        self.hosts.get(&tab_id)
    }

    pub fn close(&mut self) -> Result<(), SessionError> {
        self.closed = true;
        self.recovery.close();
        self.events.borrow_mut().clear();
        let mut first_error = None;
        for host in self.hosts.values_mut() {
            if let Err(error) = host.close() {
                first_error.get_or_insert(SessionError::Host(error));
            }
        }
        for hosts in self.orphaned_hosts.values_mut() {
            for host in hosts {
                if let Err(error) = host.close() {
                    first_error.get_or_insert(SessionError::Host(error));
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        self.hosts.clear();
        self.orphaned_hosts.clear();
        self.process_observers.clear();
        if let Some(subscription) = self.browser_subscription.as_mut() {
            subscription.close()?;
        }
        self.browser_subscription.take();
        self.browser_observer.take();
        self.environment.take();
        self.pending_hosts.clear();
        self.controller_generations.clear();
        Ok(())
    }
}

impl Drop for WebViewSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod panic_tests {
    use std::rc::Rc;

    use windows::Win32::Foundation::HWND;

    use super::{SessionError, WebViewSession};
    use crate::{environment::StaApartment, protocol::PageToHost};

    #[test]
    fn only_environment_errors_disable_every_document() {
        use crate::{environment::EnvironmentError, host::HostError};
        assert!(
            SessionError::Environment(EnvironmentError::MissingEnvironment)
                .is_environment_failure()
        );
        assert!(
            !SessionError::Host(HostError::ControllerStartupFailed(-1)).is_environment_failure()
        );
        assert!(!SessionError::CallbackPanicked.is_environment_failure());
    }

    /// A tab failure is reported once, and the failed tab is no longer
    /// retried until the environment is recreated or the tab is removed.
    #[test]
    fn a_failed_tab_is_reported_once_and_not_retried() {
        use super::TabFailure;
        use crate::host::HostError;
        let apartment = Rc::new(StaApartment::enter().unwrap());
        let observer: Rc<dyn Fn(PageToHost)> = Rc::new(|_| {});
        let mut session = WebViewSession::new(
            apartment,
            HWND::default(),
            std::env::temp_dir().join("marknexia-failed-tab-test"),
            Rc::downgrade(&observer),
        );
        let failure = TabFailure::ControllerStartup(HostError::ControllerStartupFailed(-1));
        session.fail_tab(3, failure);
        session.fail_tab(3, failure);
        assert_eq!(session.drain_tab_failures(), [(3, failure)]);
        assert!(session.drain_tab_failures().is_empty());
        assert!(session.failed_tabs.contains(&3));
        // Removing the tab forgets the failure.
        session.documents.insert(3, crate::probe::document(3, 1));
        session.remove_document(3).unwrap();
        assert!(!session.failed_tabs.contains(&3));
    }

    /// A completion lost to a panic (recorded by the `lost` callback) still
    /// fails its tab on the next poll, so the tab never stays pending.
    #[test]
    fn a_lost_controller_completion_fails_its_tab_on_the_next_poll() {
        use super::TabFailure;
        use crate::host::HostError;
        let apartment = Rc::new(StaApartment::enter().unwrap());
        let observer: Rc<dyn Fn(PageToHost)> = Rc::new(|_| {});
        let mut session = WebViewSession::new(
            apartment,
            HWND::default(),
            std::env::temp_dir().join("marknexia-lost-host-test"),
            Rc::downgrade(&observer),
        );
        session.pending_hosts.insert(5);
        session.controller_generations.insert(5, 9);
        let mut lost = session.lost_hosts.take();
        lost.push((session.generation, 9, 5));
        session.lost_hosts.set(lost);
        session.poll().unwrap();
        assert_eq!(
            session.drain_tab_failures(),
            [(
                5,
                TabFailure::ControllerStartup(HostError::CallbackPanicked)
            )]
        );
        assert!(!session.pending_hosts.contains(&5));
        assert!(session.failed_tabs.contains(&5));
    }

    /// The session's own flag: a recorded panic is reported by the next poll,
    /// exactly once. The native test
    /// `native_controller_and_browser_exit_callbacks_share_the_session_flag`
    /// checks that controllers and subscriptions are connected to it.
    #[test]
    fn a_panic_recorded_in_the_session_flag_is_reported_once() {
        let apartment = Rc::new(StaApartment::enter().unwrap());
        let observer: Rc<dyn Fn(PageToHost)> = Rc::new(|_| {});
        let mut session = WebViewSession::new(
            apartment,
            HWND::default(),
            std::env::temp_dir().join("marknexia-panic-test"),
            Rc::downgrade(&observer),
        );
        {
            let clone = session.callback_panics.clone();
            clone.guard(|| panic!("injected callback panic"));
        }
        assert!(matches!(
            session.poll(),
            Err(SessionError::CallbackPanicked)
        ));
        assert!(session.poll().is_ok());
    }
}
#[cfg(test)]
#[allow(unsafe_code)] // Ignored native tests create a real HWND on the STA.
mod native_tests {
    use std::{
        cell::RefCell,
        rc::Rc,
        time::{Duration, Instant},
    };

    use windows::{
        Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
                WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
            },
        },
        core::w,
    };

    use super::{ProcessFailure, SessionEvent, WebViewSession};
    use crate::{environment::StaApartment, probe, protocol::PageToHost};

    type ProbeSession = (
        HWND,
        WebViewSession,
        Rc<RefCell<Vec<PageToHost>>>,
        Rc<dyn Fn(PageToHost)>,
    );

    fn open_probe() -> ProbeSession {
        let apartment = Rc::new(StaApartment::enter().unwrap());
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Marknexia native callback test"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                100,
                100,
                800,
                600,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let messages = Rc::new(RefCell::new(Vec::new()));
        let captured = Rc::clone(&messages);
        let observer: Rc<dyn Fn(PageToHost)> =
            Rc::new(move |message| captured.borrow_mut().push(message));
        let folder = std::env::temp_dir()
            .join("marknexia-webview-native-tests")
            .join(std::process::id().to_string());
        let mut session = WebViewSession::new(apartment, parent, folder, Rc::downgrade(&observer));
        // The caller retains the application observer; session callbacks
        // intentionally keep only Weak references to it.
        session.add_document(probe::document(7, 1)).unwrap();
        session.add_document(probe::document(8, 1)).unwrap();
        session.select_tab(7).unwrap();
        session.start().unwrap();
        (parent, session, messages, observer)
    }

    fn pump_until(
        session: &mut WebViewSession,
        timeout: Duration,
        condition: impl Fn(&WebViewSession) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline && !condition(session) {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
                unsafe { DispatchMessageW(&message) };
            }
            session.poll().unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            condition(session),
            "native WebView2 probe did not reach its expected state"
        );
    }

    #[test]
    #[ignore = "requires an interactive x64/ARM64 Windows host with Evergreen"]
    fn native_two_tab_resource_message_renderer_and_teardown_contract() {
        let (parent, mut session, messages, _observer) = open_probe();
        pump_until(&mut session, Duration::from_secs(30), |session| {
            session.host(7).is_some()
                && session.host(8).is_some()
                && [7, 8].into_iter().all(|tab_id| {
                    messages.borrow().iter().any(|message| {
                        matches!(message, PageToHost::Ready { tab_id: ready_tab, document_epoch: 1, .. } if *ready_tab == tab_id)
                    })
                })
        });
        assert_eq!(
            session.host(7).unwrap().document_identity().unwrap(),
            (7, 1)
        );
        assert_eq!(
            session.host(8).unwrap().document_identity().unwrap(),
            (8, 1)
        );
        assert!(session.host(7).unwrap().is_visible().unwrap());
        assert!(!session.host(8).unwrap().is_visible().unwrap());
        let stale_controller_generation = session.controller_generations[&7];
        // Simulate the callback signal after the real native resource/message
        // path is installed; the adapter must close and recreate only tab 7.
        session
            .events
            .borrow_mut()
            .push_back(SessionEvent::ProcessFailure(
                session.generation,
                stale_controller_generation,
                7,
                ProcessFailure::RendererExited,
            ));
        session.poll().unwrap();
        pump_until(&mut session, Duration::from_secs(30), |session| {
            session.host(7).is_some() && session.host(8).is_some()
        });
        assert!(session.host(7).unwrap().is_visible().unwrap());
        assert!(!session.host(8).unwrap().is_visible().unwrap());
        let replacement_generation = session.controller_generations[&7];
        assert_ne!(replacement_generation, stale_controller_generation);
        session
            .events
            .borrow_mut()
            .push_back(SessionEvent::ProcessFailure(
                session.generation,
                stale_controller_generation,
                7,
                ProcessFailure::RendererExited,
            ));
        session.poll().unwrap();
        assert_eq!(session.controller_generations[&7], replacement_generation);
        assert!(session.host(7).is_some());
        messages.borrow_mut().clear();
        session
            .events
            .borrow_mut()
            .push_back(SessionEvent::PageMessage(
                session.generation,
                stale_controller_generation,
                7,
                PageToHost::Ready {
                    protocol: 1,
                    document_epoch: 1,
                    tab_id: 7,
                },
            ));
        session.poll().unwrap();
        assert!(messages.borrow().is_empty());
        session.close().unwrap();
        assert!(session.host(7).is_none() && session.host(8).is_none());
        unsafe { DestroyWindow(parent) }.unwrap();
    }

    /// A followed link replaces the tab's document in place: the same
    /// controller navigates to the new document, the old document's
    /// messages are dropped, the new one's arrive, and the load state
    /// resets and completes. Prints the navigation timing breakdown.
    #[test]
    #[ignore = "requires an interactive x64/ARM64 Windows host with Evergreen"]
    fn native_replace_document_reuses_the_controller() {
        let (parent, mut session, messages, _observer) = open_probe();
        pump_until(&mut session, Duration::from_secs(30), |session| {
            session
                .host(7)
                .is_some_and(super::WebViewHost::document_loaded)
        });
        let controller = session.controller_generations[&7];
        // Epochs 2 to 4: the probe page; 5 to 7: a rendered wiki page.
        let rendered = |epoch: u64| {
            use marknexia_core::contracts::AppTheme;
            use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, Renderer};
            let seed = u8::try_from(epoch).unwrap();
            let identity = PageIdentity::new([seed; 16], [seed ^ 0x5A; 16]).unwrap();
            let filler: String = (1..=80)
                .map(|line| format!("Filler paragraph {line} with [a link](x{line}.md).\n\n"))
                .collect();
            let source = format!("# Page {epoch}\n\n{filler}## Details\n\n{filler}");
            let page = Renderer::new()
                .render(&source, &RenderContext::new(AppTheme::Light, identity))
                .unwrap();
            crate::policy::HostDocument::from_rendered(7, epoch, "page.md", page, &identity, false)
                .unwrap()
        };
        for epoch in 2..=7 {
            messages.borrow_mut().clear();
            let document = if epoch <= 4 {
                probe::document(7, epoch)
            } else {
                rendered(epoch)
            };
            let started = Instant::now();
            session.replace_document(document).unwrap();
            assert_eq!(session.controller_generations[&7], controller);
            assert!(!session.host(7).unwrap().document_loaded());
            pump_until(&mut session, Duration::from_secs(30), |session| {
                session
                    .host(7)
                    .is_some_and(super::WebViewHost::document_loaded)
            });
            let loaded = started.elapsed();
            assert_eq!(
                session.host(7).unwrap().document_identity().unwrap(),
                (7, epoch)
            );
            let result = Rc::new(RefCell::new(None));
            session
                .host(7)
                .unwrap()
                .evaluate_for_test(
                    "(() => { const n = performance.getEntriesByType('navigation')[0]; \
                     return n ? [n.requestStart, n.responseStart, n.responseEnd, n.domInteractive, \
                     n.domContentLoadedEventStart, n.domContentLoadedEventEnd, n.loadEventEnd] \
                     .map(Math.round).join(' ') : 'none'; })()",
                    Rc::clone(&result),
                )
                .unwrap();
            pump_until(&mut session, Duration::from_secs(10), |_| {
                result.borrow().is_some()
            });
            eprintln!(
                "replace epoch {epoch}: loaded after {} ms; navigation timing (request, response start, response end, interactive, DOMContentLoaded start, end, load) {}",
                loaded.as_millis(),
                result.borrow().as_deref().unwrap_or_default()
            );
            if epoch > 4 {
                continue;
            }
            pump_until(&mut session, Duration::from_secs(10), |_| {
                messages.borrow().iter().any(|message| {
                    matches!(message, PageToHost::Ready { tab_id: 7, document_epoch, .. } if *document_epoch == epoch)
                })
            });
            assert!(messages.borrow().iter().all(|message| !matches!(
                message,
                PageToHost::Ready { tab_id: 7, document_epoch, .. } if *document_epoch != epoch
            )));
        }
        // The earlier path, for comparison: close the controller and create
        // a new one for the next document.
        let started = Instant::now();
        session.remove_document(7).unwrap();
        session.add_document(rendered(8)).unwrap();
        session.select_tab(7).unwrap();
        pump_until(&mut session, Duration::from_secs(30), |session| {
            session
                .host(7)
                .is_some_and(super::WebViewHost::document_loaded)
        });
        eprintln!(
            "new controller per document (before): loaded after {} ms",
            started.elapsed().as_millis()
        );
        assert_ne!(session.controller_generations[&7], controller);
        session.close().unwrap();
        unsafe { DestroyWindow(parent) }.unwrap();
    }

    /// The real connection: a controller (through `share_callback_panics`)
    /// and the browser-exit subscription (through `observe_browser_exit`)
    /// record into the session flag, so `poll` reports their panics once,
    /// also after the controller is removed.
    #[test]
    #[ignore = "requires an interactive x64/ARM64 Windows host with Evergreen"]
    fn native_controller_and_browser_exit_callbacks_share_the_session_flag() {
        let (parent, mut session, _messages, _observer) = open_probe();
        pump_until(&mut session, Duration::from_secs(30), |session| {
            session.host(7).is_some() && session.browser_subscription.is_some()
        });
        let host_flag = session.host(7).unwrap().panic_flag_for_test().clone();
        session.remove_document(7).unwrap();
        host_flag.guard(|| panic!("injected controller callback panic"));
        assert!(matches!(
            session.poll(),
            Err(super::SessionError::CallbackPanicked)
        ));
        assert!(session.poll().is_ok());
        session
            .browser_subscription
            .as_ref()
            .unwrap()
            .panic_flag_for_test()
            .guard(|| panic!("injected browser-exit callback panic"));
        assert!(matches!(
            session.poll(),
            Err(super::SessionError::CallbackPanicked)
        ));
        assert!(session.poll().is_ok());
        session.close().unwrap();
        unsafe { DestroyWindow(parent) }.unwrap();
    }

    /// An injected panicking page observer: poll reports the panic once and
    /// the session keeps working.
    #[test]
    #[ignore = "requires an interactive x64/ARM64 Windows host with Evergreen"]
    fn native_panicking_page_observer_is_reported_by_poll_once() {
        let (parent, mut session, _messages, _observer) = open_probe();
        pump_until(&mut session, Duration::from_secs(30), |session| {
            session.host(7).is_some()
        });
        let panicking: Rc<dyn Fn(PageToHost)> = Rc::new(|_| panic!("injected observer panic"));
        session.page_observer = Rc::downgrade(&panicking);
        session
            .events
            .borrow_mut()
            .push_back(SessionEvent::PageMessage(
                session.generation,
                session.controller_generations[&7],
                7,
                PageToHost::Ready {
                    protocol: 1,
                    document_epoch: 1,
                    tab_id: 7,
                },
            ));
        assert!(matches!(
            session.poll(),
            Err(super::SessionError::CallbackPanicked)
        ));
        assert!(session.poll().is_ok());
        session.close().unwrap();
        unsafe { DestroyWindow(parent) }.unwrap();
    }

    #[test]
    #[ignore = "requires an interactive x64/ARM64 Windows host with Evergreen"]
    fn native_page_message_is_delivered_only_after_session_poll() {
        let (parent, mut session, messages, _observer) = open_probe();
        pump_until(&mut session, Duration::from_secs(30), |session| {
            session.host(7).is_some()
        });
        messages.borrow_mut().clear();
        session
            .events
            .borrow_mut()
            .push_back(SessionEvent::PageMessage(
                session.generation,
                session.controller_generations[&7],
                7,
                PageToHost::Ready {
                    protocol: 1,
                    document_epoch: 1,
                    tab_id: 7,
                },
            ));
        assert!(messages.borrow().is_empty());
        session.poll().unwrap();
        assert_eq!(messages.borrow().len(), 1);
        session.close().unwrap();
        unsafe { DestroyWindow(parent) }.unwrap();
    }

    /// Serves large documents from the shared buffer (no per-request copy)
    /// and reports load times. Run with `--nocapture`; set
    /// `MARKNEXIA_MEASURE_SIZES_MIB` (e.g. `8,32,128`) and
    /// `MARKNEXIA_MEASURE_COPY=1` to compare with the old HGLOBAL copy.
    #[test]
    #[ignore = "requires an interactive x64/ARM64 Windows host with Evergreen"]
    fn native_large_document_streams_from_the_shared_buffer() {
        use crate::policy::{HostDocument, MAX_DOCUMENT_BYTES};
        let apartment = Rc::new(StaApartment::enter().unwrap());
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Marknexia large document test"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                100,
                100,
                800,
                600,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let observer: Rc<dyn Fn(PageToHost)> = Rc::new(|_| {});
        let folder = std::env::temp_dir()
            .join("marknexia-webview-native-tests")
            .join(format!("large-{}", std::process::id()));
        let mut results = Vec::new();
        let copy = std::env::var_os("MARKNEXIA_MEASURE_COPY").is_some();
        crate::callbacks::COPY_FOR_MEASUREMENT.store(copy, std::sync::atomic::Ordering::Relaxed);
        let sizes: Vec<usize> = std::env::var("MARKNEXIA_MEASURE_SIZES_MIB")
            .ok()
            .map(|sizes| {
                sizes
                    .split(',')
                    .filter_map(|size| size.trim().parse::<usize>().ok())
                    .map(|mib| (mib << 20).min(MAX_DOCUMENT_BYTES - 4096))
                    .collect()
            })
            // 128 MiB (`MARKNEXIA_MEASURE_SIZES_MIB=128`) is served, but
            // Chromium did not finish parsing and layout within 90 s on the
            // measurement host, so it is not a default.
            .unwrap_or_else(|| vec![64 * 1024, 8 << 20, 32 << 20]);
        for (tab_id, target) in (21_u64..).zip(sizes) {
            let mut session = WebViewSession::new(
                Rc::clone(&apartment),
                parent,
                folder.clone(),
                Rc::downgrade(&observer),
            );
            // Paragraphs, like rendered Markdown, rather than one huge text node.
            let prefix = b"<!doctype html><title>large</title><body>";
            let suffix = b"</body>";
            let mut html = Vec::with_capacity(target);
            html.extend_from_slice(prefix);
            while html.len() + 64 + suffix.len() < target {
                html.extend_from_slice(
                    b"<p>Marknexia large-document streaming paragraph of plain text.</p>\n",
                );
            }
            html.extend_from_slice(suffix);
            let bytes = html.len();
            let document = HostDocument::from_trusted_bundle(
                tab_id,
                1,
                html,
                std::collections::BTreeMap::new(),
            )
            .unwrap();
            session.add_document(document).unwrap();
            session.select_tab(tab_id).unwrap();
            session.start().unwrap();
            pump_until(&mut session, Duration::from_secs(30), |session| {
                session.host(tab_id).is_some()
            });
            let started = Instant::now();
            let deadline = started + Duration::from_secs(90);
            let loaded = loop {
                let mut message = MSG::default();
                while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
                    unsafe { DispatchMessageW(&message) };
                }
                session.poll().unwrap();
                if session
                    .host(tab_id)
                    .is_some_and(crate::host::WebViewHost::document_loaded)
                {
                    break true;
                }
                if Instant::now() > deadline {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            eprintln!(
                "served {bytes} bytes (copy={copy}); loaded={loaded}; controller-to-NavigationCompleted {} ms",
                started.elapsed().as_millis()
            );
            results.push((bytes, loaded, started.elapsed()));
            session.close().unwrap();
        }
        for (bytes, loaded, _) in &results {
            assert!(*loaded, "{bytes}-byte document did not finish navigating");
        }
        unsafe { DestroyWindow(parent) }.unwrap();
    }

    #[test]
    #[ignore = "manual native browser-process exit required after both tabs appear"]
    fn native_browser_exit_recreates_environment_and_immutable_tabs() {
        let (parent, mut session, messages, _observer) = open_probe();
        pump_until(&mut session, Duration::from_secs(30), |session| {
            session.host(7).is_some()
                && session.host(8).is_some()
                && messages.borrow().iter().any(|message| {
                    matches!(
                        message,
                        PageToHost::Ready {
                            tab_id: 7,
                            document_epoch: 1,
                            ..
                        }
                    )
                })
        });
        let original_generation = session.generation;
        // During this bounded dwell, a tester must terminate only this probe's
        // WebView2 browser process. The environment event and controller events
        // may arrive in either order; session.poll serializes both.
        pump_until(&mut session, Duration::from_secs(60), |session| {
            session.generation > original_generation
                && session.host(7).is_some()
                && session.host(8).is_some()
        });
        assert_eq!(
            session.host(7).unwrap().document_identity().unwrap(),
            (7, 1)
        );
        assert_eq!(
            session.host(8).unwrap().document_identity().unwrap(),
            (8, 1)
        );
        session.close().unwrap();
        unsafe { DestroyWindow(parent) }.unwrap();
    }
    /// Phase 3 Mermaid runtime: the pinned runtime is served through the
    /// broker under the page nonce, a valid diagram becomes SVG, an invalid
    /// one shows the in-page error with its message, and a diagram over a
    /// limit stays an accessible source fallback without loading the runtime.
    /// A page without diagrams cannot load the runtime by script or fetch,
    /// and an init directive cannot loosen `securityLevel: 'strict'`.
    #[test]
    #[ignore = "requires an interactive x64/ARM64 Windows host with Evergreen"]
    fn native_mermaid_runtime_renders_and_falls_back() {
        use marknexia_core::contracts::AppTheme;
        use marknexia_rendering::{
            DocumentRenderer, PageIdentity, RenderContext, RenderLimits, Renderer,
        };

        use crate::policy::HostDocument;

        let apartment = Rc::new(StaApartment::enter().unwrap());
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Marknexia Mermaid runtime test"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                100,
                100,
                900,
                700,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let observer: Rc<dyn Fn(PageToHost)> = Rc::new(|_| {});
        let folder = std::env::temp_dir()
            .join("marknexia-webview-native-tests")
            .join(format!("mermaid-{}", std::process::id()));
        let mut session = WebViewSession::new(apartment, parent, folder, Rc::downgrade(&observer));
        // Tab 33 renders under a 64-byte source limit; the others under the
        // .NET defaults.
        let small = Renderer::with_engine(
            marknexia_markdown::PulldownAdapter,
            RenderLimits {
                max_diagram_source_bytes: 64,
                ..RenderLimits::default()
            },
        )
        .unwrap();
        let cases = [
            (31_u64, "```mermaid\ngraph TD\nA[Alpha]-->B[Beta]\n```\n"),
            (32, "```mermaid\nthis is not a mermaid diagram\n```\n"),
            (
                33,
                "```mermaid\ngraph TD\nA[Alpha]-->B[Beta]\nB-->C[Gamma]\nC-->D[Delta]\nD-->E[Epsilon]\n```\n",
            ),
            // No diagram: the page must not be able to load the runtime.
            (34, "# Plain\n\nNo diagrams here.\n"),
            // A directive asking for `loose` and click bindings: the bridge's
            // `securityLevel: 'strict'` must hold and the SVG stay inert.
            (
                35,
                "```mermaid\n%%{init: {'securityLevel': 'loose'}}%%\ngraph TD\nA[Alpha]-->B[Beta]\nclick A callback \"Tooltip\"\nclick B href \"javascript:alert(2)\"\n```\n",
            ),
        ];
        for (tab_id, source) in cases {
            let seed = u8::try_from(tab_id).unwrap();
            let identity = PageIdentity::new([seed; 16], [seed ^ 0x5A; 16]).unwrap();
            let context = RenderContext::new(AppTheme::Light, identity);
            let rendered = if tab_id == 33 {
                small.render(source, &context)
            } else {
                Renderer::new().render(source, &context)
            }
            .unwrap();
            let document =
                HostDocument::from_rendered(tab_id, 1, "diagram.md", rendered, &identity, false)
                    .unwrap();
            assert_eq!(
                document.loads_mermaid_runtime(),
                matches!(tab_id, 31 | 32 | 35)
            );
            session.add_document(document).unwrap();
        }
        session.select_tab(31).unwrap();
        session.start().unwrap();

        const PROBE: &str = r#"JSON.stringify((() => {
  const q = id => document.getElementById(id);
  const render = q('mermaid-1-render');
  const error = q('mermaid-1-error');
  const message = q('mermaid-1-error-msg');
  const fallback = document.querySelector('.marknexia-diagram-fallback');
  const svg = render ? render.querySelector('svg') : null;
  const unsafe = [];
  if (svg) {
    for (const element of [svg, ...svg.querySelectorAll('*')]) {
      for (const attribute of element.attributes) {
        if (/^on/i.test(attribute.name) || /javascript:/i.test(attribute.value)) {
          unsafe.push(element.tagName + ' ' + attribute.name + '=' + attribute.value);
        }
      }
    }
  }
  const config = typeof mermaid === 'undefined' ? null
    : (typeof mermaid.getConfig === 'function' ? mermaid.getConfig() : mermaid.mermaidAPI.getConfig());
  return {
    runtime: typeof mermaid,
    securityLevel: config ? config.securityLevel : null,
    svg: !!svg,
    svgText: render ? render.textContent : null,
    unsafe,
    error: error ? error.style.display : null,
    message: message ? message.textContent : null,
    fallbackRole: fallback ? fallback.getAttribute('role') : null,
    fallbackText: fallback ? fallback.textContent : null,
    injected: window.__marknexiaInjected || null
  };
})())"#;
        // Run in the diagram-free page: a `<script src>` and a `fetch` for
        // the runtime URI. CSP allows the script host, so the script request
        // reaches the broker, which must refuse it; `connect-src 'none'`
        // blocks the fetch before it leaves the page.
        const INJECT: &str = r#"(() => {
  window.__marknexiaInjected = { script: 'pending', fetch: 'pending' };
  const script = document.createElement('script');
  script.src = 'https://marknexia.assets/mermaid.min.js';
  script.onload = () => { window.__marknexiaInjected.script = 'loaded'; };
  script.onerror = () => { window.__marknexiaInjected.script = 'error'; };
  document.head.appendChild(script);
  fetch('https://marknexia.assets/mermaid.min.js').then(
    response => { window.__marknexiaInjected.fetch = 'status ' + response.status; },
    () => { window.__marknexiaInjected.fetch = 'rejected'; });
  return 'injected';
})()"#;
        let evaluate = |session: &mut WebViewSession, tab_id: u64, script: &str| {
            let result = Rc::new(RefCell::new(None));
            session
                .host(tab_id)
                .unwrap()
                .evaluate_for_test(script, Rc::clone(&result))
                .unwrap();
            pump_until(session, Duration::from_secs(10), |_| {
                result.borrow().is_some()
            });
            let json = result.borrow_mut().take().unwrap();
            assert_ne!(json, "error", "ExecuteScript failed in tab {tab_id}");
            json
        };
        let mut outcomes = std::collections::BTreeMap::new();
        for (tab_id, _) in cases {
            session.select_tab(tab_id).unwrap();
            pump_until(&mut session, Duration::from_secs(30), |session| {
                session
                    .host(tab_id)
                    .is_some_and(crate::host::WebViewHost::document_loaded)
            });
            if tab_id == 34 {
                assert_eq!(evaluate(&mut session, tab_id, INJECT), "\"injected\"");
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            let outcome = loop {
                let json = evaluate(&mut session, tab_id, PROBE);
                let text: String = serde_json::from_str(&json).unwrap();
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                let settled = match tab_id {
                    31 | 35 => value["svg"] == true || value["error"] == "block",
                    32 => value["error"] == "block",
                    34 => {
                        value["injected"]["script"] != "pending"
                            && value["injected"]["fetch"] != "pending"
                    }
                    _ => true,
                };
                if settled || Instant::now() > deadline {
                    break value;
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            eprintln!("tab {tab_id}: {outcome}");
            outcomes.insert(tab_id, outcome);
        }
        let valid = &outcomes[&31];
        assert_eq!(valid["runtime"], "object", "the pinned runtime loaded");
        assert_eq!(valid["svg"], true, "the valid diagram became SVG");
        let labels = valid["svgText"].as_str().unwrap();
        assert!(
            labels.contains("Alpha") && labels.contains("Beta"),
            "{labels}"
        );
        assert_eq!(valid["error"], "none");
        assert_eq!(valid["securityLevel"], "strict");

        let invalid = &outcomes[&32];
        assert_eq!(invalid["runtime"], "object");
        assert_eq!(invalid["svg"], false);
        assert_eq!(invalid["error"], "block", "the in-page diagnostic is shown");
        assert!(!invalid["message"].as_str().unwrap().trim().is_empty());

        let limited = &outcomes[&33];
        assert_eq!(limited["runtime"], "undefined", "no runtime for a fallback");
        assert_eq!(limited["fallbackRole"], "note");
        let fallback = limited["fallbackText"].as_str().unwrap();
        assert!(
            fallback.contains("Mermaid diagram was not rendered."),
            "{fallback}"
        );
        assert!(
            fallback.contains("diagram source limit exceeded"),
            "{fallback}"
        );
        assert!(
            fallback.contains("A[Alpha]--&gt;B[Beta]") || fallback.contains("A[Alpha]-->B[Beta]")
        );

        let plain = &outcomes[&34];
        assert_eq!(
            plain["runtime"], "undefined",
            "the injected runtime never ran"
        );
        assert_eq!(
            plain["injected"]["script"], "error",
            "the broker refused the injected script request"
        );
        assert_eq!(
            plain["injected"]["fetch"], "rejected",
            "the page cannot fetch the runtime"
        );

        let directive = &outcomes[&35];
        assert_eq!(directive["runtime"], "object");
        assert_eq!(
            directive["securityLevel"], "strict",
            "an init directive cannot loosen the security level"
        );
        assert_eq!(directive["svg"], true, "{directive}");
        assert_eq!(
            directive["unsafe"],
            serde_json::json!([]),
            "no event handler or javascript: URL in the SVG"
        );
        session.close().unwrap();
        unsafe { DestroyWindow(parent) }.unwrap();
    }
}
