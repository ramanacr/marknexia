//! Portable shell state. HWND and COM ownership remain in platform adapters.

use crate::{
    keyboard::ShellCommand,
    tabs::{OpenError, TabId, TabStore},
    theme::ThemePreference,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusSurface {
    CommandBar,
    TabStrip,
    Repository,
    Document,
    FindBar,
    Status,
}

impl FocusSurface {
    const ORDER: [Self; 6] = [
        Self::CommandBar,
        Self::TabStrip,
        Self::Repository,
        Self::Document,
        Self::FindBar,
        Self::Status,
    ];

    fn next(self) -> Self {
        let current = Self::ORDER
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or_default();
        Self::ORDER[(current + 1) % Self::ORDER.len()]
    }
}

pub struct AppState {
    tabs: TabStore,
    focus: FocusSurface,
    focus_visible: bool,
    sidebar_visible: bool,
    find_bar_visible: bool,
    theme: ThemePreference,
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tabs: TabStore::new(),
            focus: FocusSurface::CommandBar,
            focus_visible: false,
            sidebar_visible: true,
            find_bar_visible: false,
            theme: ThemePreference::System,
        }
    }

    pub fn open_tab(&mut self, title: impl Into<String>) -> Result<TabId, OpenError> {
        self.tabs.open(title)
    }

    #[must_use]
    pub fn active_tab(&self) -> Option<TabId> {
        self.tabs.active_id()
    }

    #[must_use]
    pub fn tabs(&self) -> &TabStore {
        &self.tabs
    }

    pub fn select_tab(&mut self, tab_id: TabId) -> bool {
        self.tabs.select(tab_id)
    }

    pub fn apply_command(&mut self, command: ShellCommand) -> bool {
        match command {
            ShellCommand::NextTab => self.tabs.select_next().is_some(),
            ShellCommand::PreviousTab => self.tabs.select_previous().is_some(),
            ShellCommand::CloseTab => self
                .tabs
                .active_id()
                .is_some_and(|active| self.tabs.close(active)),
            ShellCommand::CycleFocus => {
                self.focus = self.focus.next();
                self.focus_visible = true;
                true
            }
        }
    }

    #[must_use]
    pub fn focused_surface(&self) -> FocusSurface {
        self.focus
    }

    #[must_use]
    pub fn focus_is_visible(&self) -> bool {
        self.focus_visible
    }

    pub fn note_keyboard_input(&mut self) {
        self.focus_visible = true;
    }

    pub fn note_pointer_input(&mut self) {
        self.focus_visible = false;
    }

    #[must_use]
    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    #[must_use]
    pub fn find_bar_visible(&self) -> bool {
        self.find_bar_visible
    }

    #[must_use]
    pub fn theme(&self) -> ThemePreference {
        self.theme
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
