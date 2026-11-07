//! Per-tab document history with .NET-compatible back/forward stack semantics.

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub document_path: String,
    pub fragment: Option<String>,
    pub scroll_top: f64,
}

impl HistoryEntry {
    pub fn new(path: &str, fragment: Option<&str>, scroll_top: impl Into<f64>) -> Self {
        Self {
            document_path: path.to_owned(),
            fragment: fragment.map(str::to_owned),
            scroll_top: scroll_top.into(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct NavigationHistory {
    back: Vec<HistoryEntry>,
    forward: Vec<HistoryEntry>,
}

impl NavigationHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn back_len(&self) -> usize {
        self.back.len()
    }

    pub fn forward_len(&self) -> usize {
        self.forward.len()
    }

    pub fn push(&mut self, previous: HistoryEntry) {
        self.back.push(previous);
        self.forward.clear();
    }

    pub fn go_back(&mut self, current: HistoryEntry) -> Option<HistoryEntry> {
        let previous = self.back.pop()?;
        self.forward.push(current);
        Some(previous)
    }

    pub fn go_forward(&mut self, current: HistoryEntry) -> Option<HistoryEntry> {
        let next = self.forward.pop()?;
        self.back.push(current);
        Some(next)
    }

    pub fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }
}
