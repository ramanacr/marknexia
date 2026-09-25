//! Stable tab identity and selection, independent of HWND/controller lifetime.

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TabId(u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tab {
    id: TabId,
    title: String,
}

impl Tab {
    #[must_use]
    pub fn id(&self) -> TabId {
        self.id
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }
}

impl TabId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenError {
    IdExhausted,
}

pub struct TabStore {
    tabs: Vec<Tab>,
    active: Option<TabId>,
    next_id: u64,
}

impl TabStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active: None,
            next_id: 1,
        }
    }

    pub fn open(&mut self, title: impl Into<String>) -> Result<TabId, OpenError> {
        let following = self.next_id.checked_add(1).ok_or(OpenError::IdExhausted)?;
        let id = TabId(self.next_id);
        self.next_id = following;
        self.tabs.push(Tab {
            id,
            title: title.into(),
        });
        self.active = Some(id);
        Ok(id)
    }

    pub fn select(&mut self, id: TabId) -> bool {
        if self.tabs.iter().any(|tab| tab.id == id) {
            self.active = Some(id);
            true
        } else {
            false
        }
    }

    pub fn select_next(&mut self) -> Option<TabId> {
        self.select_adjacent(false)
    }

    pub fn select_previous(&mut self) -> Option<TabId> {
        self.select_adjacent(true)
    }

    pub fn close(&mut self, id: TabId) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return false;
        };
        self.tabs.remove(index);
        if self.active == Some(id) {
            self.active = self
                .tabs
                .get(index)
                .or_else(|| self.tabs.last())
                .map(|tab| tab.id);
        }
        true
    }

    pub fn reorder(&mut self, id: TabId, destination: usize) -> bool {
        if destination >= self.tabs.len() {
            return false;
        }
        let Some(source) = self.tabs.iter().position(|tab| tab.id == id) else {
            return false;
        };
        let tab = self.tabs.remove(source);
        self.tabs.insert(destination, tab);
        true
    }

    fn select_adjacent(&mut self, reverse: bool) -> Option<TabId> {
        let active = self.active?;
        let current = self.tabs.iter().position(|tab| tab.id == active)?;
        let following = if reverse {
            (current + self.tabs.len() - 1) % self.tabs.len()
        } else {
            (current + 1) % self.tabs.len()
        };
        let id = self.tabs[following].id;
        self.active = Some(id);
        Some(id)
    }

    #[must_use]
    pub fn active_id(&self) -> Option<TabId> {
        self.active
    }

    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }
}

impl Default for TabStore {
    fn default() -> Self {
        Self::new()
    }
}
