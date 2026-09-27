//! STA-owned controller fleet. COM callbacks enqueue events; the host pumps them.

use std::{
    cell::RefCell,
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
    policy::{DocumentError, HostDocument},
    protocol::PageToHost,
    recovery::{RecoveryAction, RecoveryCoordinator},
};

enum SessionEvent {
    EnvironmentCreated(u64, Result<WebViewEnvironment, EnvironmentError>),
    HostCreated(u64, u64, u64, Result<WebViewHost, HostError>),
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
        if let Some(mut host) = self.hosts.remove(&tab_id)
            && let Err(error) = host.close()
        {
            self.orphaned_hosts.entry(tab_id).or_default().push(host);
            return Err(SessionError::Host(error));
        }
        Ok(())
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
        if self.hosts.contains_key(&tab_id) || !self.pending_hosts.insert(tab_id) {
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
        let result = environment.create_host_async(
            self.parent,
            Box::new(move |result| {
                events.borrow_mut().push_back(SessionEvent::HostCreated(
                    generation,
                    controller_generation,
                    tab_id,
                    result,
                ));
            }),
        );
        if let Err(error) = result {
            self.pending_hosts.remove(&tab_id);
            self.controller_generations.remove(&tab_id);
            return Err(SessionError::Host(error));
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
        if self.environment.is_some()
            && !self.teardown_pending
            && !self.environment_recreate_pending
        {
            if !self.browser_recovering {
                self.retry_renderer_recreation()?;
            }
            self.close_orphaned_hosts()?;
            for tab_id in self.documents.keys().copied().collect::<Vec<_>>() {
                if !self.hosts.contains_key(&tab_id)
                    && !self.orphaned_hosts.contains_key(&tab_id)
                    && !self.recreating_tabs.contains(&tab_id)
                {
                    self.begin_host(tab_id)?;
                }
            }
        }
        self.finish_environment_restore()?;
        if let Some(tab_id) = self.active_tab_restore_pending {
            self.restore_active_tab(tab_id)?;
        }
        if self.visibility_pending {
            self.apply_selected_visibility()?;
        }
        Ok(())
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
                let subscription = environment.observe_browser_exit(Rc::downgrade(&observer))?;
                self.browser_observer = Some(observer);
                self.browser_subscription = Some(subscription);
                self.environment = Some(environment);
                for tab_id in self.documents.keys().copied().collect::<Vec<_>>() {
                    self.begin_host(tab_id)?;
                }
                self.finish_environment_restore()?;
            }
            SessionEvent::HostCreated(generation, controller_generation, tab_id, result) => {
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
                        return Err(SessionError::Host(error));
                    }
                };
                host.set_bounds(self.viewport)?;
                host.set_default_background(self.background)?;
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
                    return Err(SessionError::Host(error));
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
                    observer(message);
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
        self.close_orphaned_hosts()?;
        self.teardown_pending = false;
        let action = self.recovery.controllers_closed();
        self.apply_recovery_action(action)
    }

    fn retry_renderer_recreation(&mut self) -> Result<(), SessionError> {
        for tab_id in self.recreating_tabs.iter().copied().collect::<Vec<_>>() {
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

    fn close_orphaned_hosts(&mut self) -> Result<(), SessionError> {
        for tab_id in self.orphaned_hosts.keys().copied().collect::<Vec<_>>() {
            if let Some(hosts) = self.orphaned_hosts.get_mut(&tab_id) {
                for host in hosts {
                    host.close()?;
                }
            }
            self.orphaned_hosts.remove(&tab_id);
        }
        Ok(())
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
            && self.hosts.len() == self.documents.len()
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
}
