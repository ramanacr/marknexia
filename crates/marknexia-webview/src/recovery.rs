//! Pure recovery decisions; the STA adapter executes actions and reports completion.

use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    None,
    RecreateController { tab_id: u64 },
    CloseAllControllers,
    RecreateEnvironment,
    RestoreActiveTab { tab_id: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserStage {
    Healthy,
    ClosingControllers,
    WaitingForExit,
    RecreatingEnvironment,
    RestoringTab,
    Closed,
}

#[derive(Debug)]
pub struct RecoveryCoordinator {
    active_tab_id: Option<u64>,
    renderer_pending: BTreeSet<u64>,
    browser_stage: BrowserStage,
    browser_exit_observed: bool,
}

impl RecoveryCoordinator {
    pub fn is_healthy(&self) -> bool {
        self.browser_stage == BrowserStage::Healthy
    }

    #[must_use]
    pub fn new(active_tab_id: Option<u64>) -> Self {
        Self {
            active_tab_id,
            renderer_pending: BTreeSet::new(),
            browser_stage: BrowserStage::Healthy,
            browser_exit_observed: false,
        }
    }

    pub fn set_active_tab(&mut self, tab_id: Option<u64>) {
        if self.browser_stage == BrowserStage::Healthy {
            self.active_tab_id = tab_id;
        }
    }

    pub fn record_renderer_failure(&mut self, tab_id: u64) -> RecoveryAction {
        if self.browser_stage == BrowserStage::Healthy && self.renderer_pending.insert(tab_id) {
            RecoveryAction::RecreateController { tab_id }
        } else {
            RecoveryAction::None
        }
    }

    pub fn renderer_restored(&mut self, tab_id: u64) {
        if self.browser_stage == BrowserStage::Healthy {
            self.renderer_pending.remove(&tab_id);
        }
    }

    pub fn record_browser_exit(&mut self) -> RecoveryAction {
        if self.browser_stage != BrowserStage::Healthy {
            return RecoveryAction::None;
        }
        self.renderer_pending.clear();
        self.browser_stage = BrowserStage::ClosingControllers;
        RecoveryAction::CloseAllControllers
    }

    /// The environment-level exit event can arrive before or after each
    /// controller's ProcessFailed callback. Recreate only once the controllers
    /// are closed and the old browser process has actually exited.
    pub fn browser_process_exited(&mut self) -> RecoveryAction {
        match self.browser_stage {
            BrowserStage::Healthy => {
                self.browser_exit_observed = true;
                self.renderer_pending.clear();
                self.browser_stage = BrowserStage::ClosingControllers;
                RecoveryAction::CloseAllControllers
            }
            BrowserStage::ClosingControllers => {
                self.browser_exit_observed = true;
                RecoveryAction::None
            }
            BrowserStage::WaitingForExit => {
                self.browser_exit_observed = true;
                self.browser_stage = BrowserStage::RecreatingEnvironment;
                RecoveryAction::RecreateEnvironment
            }
            BrowserStage::RecreatingEnvironment
            | BrowserStage::RestoringTab
            | BrowserStage::Closed => RecoveryAction::None,
        }
    }

    pub fn controllers_closed(&mut self) -> RecoveryAction {
        if self.browser_stage != BrowserStage::ClosingControllers {
            return RecoveryAction::None;
        }
        if self.browser_exit_observed {
            self.browser_stage = BrowserStage::RecreatingEnvironment;
            RecoveryAction::RecreateEnvironment
        } else {
            self.browser_stage = BrowserStage::WaitingForExit;
            RecoveryAction::None
        }
    }

    pub fn environment_restored(&mut self) -> RecoveryAction {
        if self.browser_stage != BrowserStage::RecreatingEnvironment {
            return RecoveryAction::None;
        }
        if let Some(tab_id) = self.active_tab_id {
            self.browser_stage = BrowserStage::RestoringTab;
            RecoveryAction::RestoreActiveTab { tab_id }
        } else {
            self.browser_stage = BrowserStage::Healthy;
            self.browser_exit_observed = false;
            RecoveryAction::None
        }
    }

    pub fn active_tab_restored(&mut self, tab_id: u64) {
        if self.browser_stage == BrowserStage::RestoringTab && self.active_tab_id == Some(tab_id) {
            self.browser_stage = BrowserStage::Healthy;
            self.browser_exit_observed = false;
        }
    }

    pub fn close(&mut self) {
        self.renderer_pending.clear();
        self.browser_stage = BrowserStage::Closed;
    }
}
