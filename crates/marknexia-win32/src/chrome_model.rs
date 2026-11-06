//! Portable shell-chrome decisions ported from the .NET `MainWindow`:
//! sidebar modes and width, the outline, the repository tree, per-tab anchor
//! history, command shortcuts, the diagnostics bar and dialog text. No HWND
//! or COM; the Win32 adapter lives in `window::chrome`.

use std::path::{Path, PathBuf};

use marknexia_core::{
    contracts::DiagnosticSeverity,
    settings::{DEFAULT_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH},
    text::eq_ignore_case,
};
use marknexia_files::repository::{RepositoryTree, RepositoryTreeNode};
use marknexia_navigation::history::{HistoryEntry, NavigationHistory};

use crate::theme::ThemePreference;

/// The sidebar views. `Outline` and `Repository` are the .NET
/// `SidebarModeSelector` tabs (persisted as `SidebarMode` 0 and 1). `Recent`
/// shows the .NET start page's recent files list in the sidebar (decision
/// CHROME-1); it is not persisted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SidebarMode {
    Outline,
    Repository,
    Recent,
}

impl SidebarMode {
    pub const ALL: [Self; 3] = [Self::Outline, Self::Repository, Self::Recent];

    /// .NET `Math.Clamp(SidebarMode, 0, 1)`.
    #[must_use]
    pub fn from_setting(value: i32) -> Self {
        if value >= 1 {
            Self::Repository
        } else {
            Self::Outline
        }
    }

    /// The `SidebarMode` value to save; `None` for the Rust-only view.
    #[must_use]
    pub fn setting_value(self) -> Option<i32> {
        match self {
            Self::Outline => Some(0),
            Self::Repository => Some(1),
            Self::Recent => None,
        }
    }

    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Outline => 0,
            Self::Repository => 1,
            Self::Recent => 2,
        }
    }

    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// The selector tab text (.NET `TabViewItem.Header`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Outline => "Outline",
            Self::Repository => "Repository",
            Self::Recent => "Recent files",
        }
    }

    /// The heading above the list (.NET `DocumentMapHeader` and
    /// `RepositoryHeader`).
    #[must_use]
    pub fn header(self) -> &'static str {
        match self {
            Self::Outline => "Document Map",
            Self::Repository => "Repository",
            Self::Recent => "Recent files",
        }
    }

    /// The list's accessible name (.NET `AutomationProperties.Name`).
    #[must_use]
    pub fn list_name(self) -> &'static str {
        match self {
            Self::Outline => "Document outline",
            Self::Repository => "Repository files",
            Self::Recent => "Recent Markdown files",
        }
    }
}

/// Keyboard steps for the sidebar splitter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SidebarResize {
    Narrower,
    Wider,
    Minimum,
    Maximum,
}

/// DIPs moved by one arrow key on the splitter.
pub const SIDEBAR_RESIZE_STEP: f64 = 16.0;

/// .NET `Math.Clamp(width, MinimumSidebarWidth, MaximumSidebarWidth)`; NaN
/// becomes the default width.
#[must_use]
pub fn clamp_sidebar_width(width: f64) -> f64 {
    if width.is_nan() {
        DEFAULT_SIDEBAR_WIDTH
    } else {
        width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
    }
}

#[must_use]
pub fn resize_sidebar(width: f64, step: SidebarResize) -> f64 {
    clamp_sidebar_width(match step {
        SidebarResize::Narrower => width - SIDEBAR_RESIZE_STEP,
        SidebarResize::Wider => width + SIDEBAR_RESIZE_STEP,
        SidebarResize::Minimum => MIN_SIDEBAR_WIDTH,
        SidebarResize::Maximum => MAX_SIDEBAR_WIDTH,
    })
}

/// The splitter key for a virtual key, if it resizes the sidebar.
#[must_use]
pub fn sidebar_resize_key(virtual_key: u16) -> Option<SidebarResize> {
    match virtual_key {
        0x25 => Some(SidebarResize::Narrower),
        0x27 => Some(SidebarResize::Wider),
        0x24 => Some(SidebarResize::Minimum),
        0x23 => Some(SidebarResize::Maximum),
        _ => None,
    }
}

/// Whole DIPs for layout; the stored width keeps its fraction.
#[must_use]
pub fn sidebar_width_dip(width: f64) -> u32 {
    // The clamp keeps the value in 220..=520, so the cast cannot truncate.
    clamp_sidebar_width(width).round() as u32
}

/// One heading of the active document (.NET `OutlineItemViewModel`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutlineEntry {
    pub text: String,
    pub slug: String,
    pub level: u8,
}

impl OutlineEntry {
    /// .NET `Margin = (Level - 1) * 14` on the left.
    #[must_use]
    pub fn indent_dip(&self) -> u32 {
        u32::from(self.level.saturating_sub(1)) * 14
    }
}

/// Commands from the toolbar, the menu bar and the keyboard (.NET
/// `RegisterKeyboardAccelerators` and the `*_Click` handlers). Find and the
/// tab chords stay in `keyboard::route_key`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChromeCommand {
    OpenFile,
    OpenFolder,
    Reload,
    Back,
    Forward,
    CloseTab,
    ToggleSidebar,
    ToggleRemoteAssets,
    SetTheme(ThemePreference),
    ShowHelp,
    ShowAbout,
}

const VK_LEFT: u16 = 0x25;
const VK_RIGHT: u16 = 0x27;
const VK_O: u16 = 0x4F;
const VK_R: u16 = 0x52;
const VK_T: u16 = 0x54;
const VK_F5: u16 = 0x74;

/// The .NET accelerators: Ctrl+O and Ctrl+T open a file, Ctrl+Shift+O opens
/// a folder, F5 and Ctrl+R reload, Alt+Left and Alt+Right go back and
/// forward. Ctrl+W, Ctrl+Tab and Ctrl+Shift+Tab are routed by
/// `keyboard::route_key`.
#[must_use]
pub fn route_chrome_key(
    virtual_key: u16,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Option<ChromeCommand> {
    match (virtual_key, ctrl, shift, alt) {
        (VK_O, true, false, false) | (VK_T, true, false, false) => Some(ChromeCommand::OpenFile),
        (VK_O, true, true, false) => Some(ChromeCommand::OpenFolder),
        (VK_F5, false, false, false) | (VK_R, true, false, false) => Some(ChromeCommand::Reload),
        (VK_LEFT, false, false, true) => Some(ChromeCommand::Back),
        (VK_RIGHT, false, false, true) => Some(ChromeCommand::Forward),
        _ => None,
    }
}

/// The message shown by the diagnostics bar (.NET `ShowDiagnostic`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticMessage {
    pub message: String,
    pub severity: DiagnosticSeverity,
}

impl DiagnosticMessage {
    #[must_use]
    pub fn new(message: impl Into<String>, severity: DiagnosticSeverity) -> Self {
        Self {
            message: message.into(),
            severity,
        }
    }

    /// The severity as the WinUI `InfoBarSeverity` names it.
    #[must_use]
    pub fn severity_label(&self) -> &'static str {
        match self.severity {
            DiagnosticSeverity::Info => "Informational",
            DiagnosticSeverity::Warning => "Warning",
            DiagnosticSeverity::Error => "Error",
        }
    }

    /// The accessible name: severity first, then the message, so a screen
    /// reader announces both (the colour and glyph are not announced).
    #[must_use]
    pub fn accessible_name(&self) -> String {
        format!("{}: {}", self.severity_label(), self.message)
    }
}

/// The diagnostics bar: one message at a time, the newest replacing the
/// previous, open until dismissed (.NET `InfoBar.IsOpen`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticsBar {
    current: Option<DiagnosticMessage>,
}

impl DiagnosticsBar {
    pub fn show(&mut self, message: impl Into<String>, severity: DiagnosticSeverity) {
        self.current = Some(DiagnosticMessage::new(message, severity));
    }

    /// The close button. Returns whether the bar was open.
    pub fn dismiss(&mut self) -> bool {
        self.current.take().is_some()
    }

    #[must_use]
    pub fn current(&self) -> Option<&DiagnosticMessage> {
        self.current.as_ref()
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.current.is_some()
    }
}

/// .NET `RemoteAssetsMenuItem_Click` messages.
#[must_use]
pub fn remote_assets_notice(enabled: bool) -> &'static str {
    if enabled {
        "Remote images are enabled for documents rendered from now on."
    } else {
        "Remote images are blocked by default."
    }
}

pub const HELP_TITLE: &str = "Marknexia help";
pub const ABOUT_TITLE: &str = "About Marknexia";

/// .NET `ShowHelp_Click` text.
#[must_use]
pub fn help_text() -> String {
    [
        "Open a Markdown file or drop a file/folder onto the window. Repository mode shows Markdown files and supported assets; select a heading in the document map to jump to it.",
        "",
        "Keyboard shortcuts",
        "Ctrl+O  Open file",
        "Ctrl+Shift+O  Open repository",
        "Ctrl+T  Open a file picker for a new tab",
        "Ctrl+F  Find",
        "Enter / Shift+Enter  Next / previous match",
        "Alt+Left / Alt+Right  Back / forward",
        "Ctrl+Tab / Ctrl+Shift+Tab  Next / previous tab",
        "Ctrl+W  Close tab",
        "F5 / Ctrl+R  Reload",
        "Esc  Close search",
        "",
        "Links stay inside the repository sandbox when enabled. External links require an explicit handoff to the system browser. Rendering assets are bundled locally.",
    ]
    .join("\n")
}

/// .NET `ShowAbout_Click` text.
#[must_use]
pub fn about_text(version: &str) -> String {
    format!(
        "Marknexia\nRead. Explore. Understand.\nGitHub-style Markdown. Native on Windows.\n\nVersion {version}\nOffline-first rendering \u{2022} Secure local navigation"
    )
}

/// Ordinal case-insensitive path equality, as .NET compares open tabs.
#[must_use]
pub fn same_document_path(left: &Path, right: &Path) -> bool {
    eq_ignore_case(&left.to_string_lossy(), &right.to_string_lossy())
}

/// .NET `string.Equals(a, b, OrdinalIgnoreCase)` for optional fragments.
fn same_fragment(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => eq_ignore_case(left, right),
        _ => false,
    }
}

/// One tab's document location and back/forward history (.NET
/// `DocumentTabState.FilePath`, `CurrentFragment` and `History`).
#[derive(Clone, Debug)]
pub struct TabLocation {
    path: PathBuf,
    fragment: Option<String>,
    history: NavigationHistory,
}

impl TabLocation {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            fragment: None,
            history: NavigationHistory::new(),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    #[must_use]
    pub fn can_go_back(&self) -> bool {
        self.history.can_go_back()
    }

    #[must_use]
    pub fn can_go_forward(&self) -> bool {
        self.history.can_go_forward()
    }

    fn entry(&self) -> HistoryEntry {
        HistoryEntry::new(&self.path.to_string_lossy(), self.fragment.as_deref(), 0)
    }

    /// An outline jump in the open document. Returns whether the location
    /// changed; only then is the previous location pushed (.NET
    /// `locationChanged && recordHistory`).
    pub fn navigate_to_anchor(&mut self, slug: &str) -> bool {
        self.navigate_to(Some(slug))
    }

    /// A location in the open document: an anchor, or the top (`None`,
    /// .NET `scrollToTop`). Returns whether the location changed.
    pub fn navigate_to(&mut self, fragment: Option<&str>) -> bool {
        if same_fragment(self.fragment.as_deref(), fragment) {
            return false;
        }
        let previous = self.entry();
        self.history.push(previous);
        self.fragment = fragment.map(str::to_owned);
        true
    }

    /// Back: the entry to show, now current. `None` when there is no
    /// history.
    pub fn go_back(&mut self) -> Option<HistoryEntry> {
        let current = self.entry();
        let previous = self.history.go_back(current)?;
        self.fragment.clone_from(&previous.fragment);
        Some(previous)
    }

    /// Forward: the entry to show, now current.
    pub fn go_forward(&mut self) -> Option<HistoryEntry> {
        let current = self.entry();
        let next = self.history.go_forward(current)?;
        self.fragment.clone_from(&next.fragment);
        Some(next)
    }
}

/// One node of the flattened repository tree. Children are indices into
/// [`TreeArena::nodes`], so the tree view can fill folders on expansion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub is_folder: bool,
    pub children: Vec<usize>,
}

/// The repository tree flattened iteratively (deep trees never recurse).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TreeArena {
    nodes: Vec<TreeNode>,
    incomplete: bool,
}

impl TreeArena {
    #[must_use]
    pub fn from_tree(tree: RepositoryTree) -> Self {
        let incomplete = tree.limits_hit.any();
        let mut nodes = Vec::new();
        // (node, parent index)
        let mut stack: Vec<(RepositoryTreeNode, Option<usize>)> = vec![(tree.root, None)];
        while let Some((mut node, parent)) = stack.pop() {
            let index = nodes.len();
            let children = std::mem::take(&mut node.children);
            let name = if node.display_name.is_empty() {
                node.full_path.to_string_lossy().into_owned()
            } else {
                node.display_name
            };
            nodes.push(TreeNode {
                name,
                path: node.full_path,
                is_folder: node.is_folder,
                children: Vec::with_capacity(children.len()),
            });
            if let Some(parent) = parent {
                nodes[parent].children.push(index);
            }
            // Reverse so children are visited, and numbered, in order.
            stack.extend(children.into_iter().rev().map(|child| (child, Some(index))));
        }
        Self { nodes, incomplete }
    }

    #[must_use]
    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    #[must_use]
    pub fn node(&self, index: usize) -> Option<&TreeNode> {
        self.nodes.get(index)
    }

    /// The root (index 0), if the tree has one.
    #[must_use]
    pub fn root(&self) -> Option<&TreeNode> {
        self.nodes.first()
    }

    /// Scan limits shortened the tree, so it may be incomplete.
    #[must_use]
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }
}

/// The repository header text, marked when the tree may be incomplete.
#[must_use]
pub fn repository_header(incomplete: bool) -> &'static str {
    if incomplete {
        "Repository (incomplete)"
    } else {
        "Repository"
    }
}

/// The diagnostics text for a tree shortened by scan limits.
pub const REPOSITORY_INCOMPLETE_NOTICE: &str =
    "The repository tree is incomplete: the folder is larger than the scan limits.";
