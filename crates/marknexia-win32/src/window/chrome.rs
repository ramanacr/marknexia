//! Shell chrome ported from the .NET `MainWindow`: toolbar and menu bar,
//! sidebar (outline, repository tree, recent files, splitter), diagnostics
//! bar and the settings that persist them.
//!
//! `window.rs` calls into this module through a few hooks (`install`,
//! `after_startup`, `handle_message`, `translate_message`, `note_render`,
//! `filter_render_outcome`, `layout_inputs`, `layout_children`,
//! `tabs_changed`, `theme_changed`, `after_poll`, `route_accelerator`).
//!
//! Invariants kept from `window.rs`:
//! - `AppState` stays the one `Rc<RefCell<_>>` in `GWLP_USERDATA`;
//! - no `RefCell` borrow is held while a control is sent a message, a dialog
//!   runs, or WebView2 is called: data is copied out first;
//! - control notifications (`WM_COMMAND`, `WM_NOTIFY`) only queue an action
//!   and post `WM_CHROME_ACTION`; the action runs from the message loop, as
//!   the Open dialog does (`WM_OPEN_DIALOG`).

mod updater;

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::c_void,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use marknexia_core::{contracts::DiagnosticSeverity, settings::ThemePreference as StoredTheme};
use marknexia_files::{
    path::GatedPath,
    repository::{ScanError, ScanLimits, build_repository_tree, detect_repository_root},
    settings_store::SettingsStore,
};
use marknexia_webview::host::ShellAccelerator;
use windows::Win32::{
    System::{
        Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        Variant::VARIANT,
    },
    UI::{
        Accessibility::{
            CAccPropServices, IAccPropServices, IsContentElement_Property_GUID,
            IsControlElement_Property_GUID, Name_Property_GUID, PROPID_ACC_NAME,
        },
        WindowsAndMessaging::{CHILDID_SELF, OBJID_CLIENT},
    },
};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{
            DeleteObject, HDC, HFONT, HGDIOBJ, InvalidateRect, SetBkColor, SetTextColor,
        },
        UI::{
            Controls::{
                DRAWITEMSTRUCT, HTREEITEM, MEASUREITEMSTRUCT, NMHDR, NMTREEVIEWW, SetWindowTheme,
            },
            Input::KeyboardAndMouse::{
                EnableWindow, GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_MENU, VK_RETURN,
                VK_SHIFT, VK_SPACE, VK_TAB,
            },
            WindowsAndMessaging::{
                CheckMenuItem, GetNextDlgTabItem, GetWindowThreadProcessId, HMENU, HWND_TOP,
                IsChild, IsWindow, MF_BYCOMMAND, MF_CHECKED, MF_UNCHECKED, MSG, PostMessageW,
                SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SendMessageW,
                SetWindowPos, SetWindowTextW, ShowWindow, WM_APP, WM_COMMAND, WM_CTLCOLOREDIT,
                WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DRAWITEM, WM_KEYDOWN, WM_MEASUREITEM,
                WM_MENUCHAR, WM_NOTIFY, WM_PAINT, WM_SYSKEYDOWN,
            },
        },
    },
    core::{PCWSTR, w},
};

use super::{
    AppState, ShellControls, WM_OPEN_DIALOG, WindowError, app_state_handle,
    diagnostics::{self, DiagnosticsControls},
    dialogs, execute_command, menu_theme, move_child, native_color, open_document, palette, reflow,
    refresh_status, restore_session, select_tab_id, settings_bridge, shell_ops, sidebar,
    start_render, style, take_session,
    toolbar::{self, Toolbar},
    update_theme,
};
use crate::{
    chrome_model::{
        ANCHOR_COPY_FAILED, ChromeCommand, DIAGNOSTICS_COPIED, DiagnosticsBar, DiagnosticsFacts,
        OutlineEntry, REPOSITORY_INCOMPLETE_NOTICE, SBOM_COPIED, SBOM_FAILED, SBOM_MISSING,
        SidebarMode, SidebarResize, TROUBLESHOOTING_FAILED, TabLocation, TreeArena, TreeNode,
        anchor_clipboard_text, anchor_copied_notice, clamp_sidebar_width, compact_transition,
        diagnostics_report, dotnet_architecture, remote_assets_notice, repository_header,
        resize_sidebar, reveal_failed_notice, route_chrome_key, same_document_path,
        sidebar_width_dip, text_diagrams_notice,
    },
    documents::{OpenError, check_document_path},
    keyboard::ShellCommand,
    layout::{Dip, PixelRect, ShellLayout},
    theme::{ColorSpec, EffectiveTheme, SystemColorRole, ThemePreference},
};

/// Runs queued chrome actions outside the control notification.
const WM_CHROME_ACTION: u32 = WM_APP + 0x40;
/// A repository worker queued a result.
const WM_CHROME_WORKER: u32 = WM_APP + 0x41;

pub(super) const ID_TOGGLE_SIDEBAR: usize = 2001;
pub(super) const ID_BACK: usize = 2002;
pub(super) const ID_FORWARD: usize = 2003;
pub(super) const ID_RELOAD: usize = 2004;
pub(super) const ID_OPEN_FOLDER: usize = 2005;
pub(super) const ID_THEME_LABEL: usize = 2006;
pub(super) const ID_THEME: usize = 2007;
/// input: the find toggle (.NET `SearchToggleButton`); handled by `input`.
pub(super) const ID_FIND: usize = 2008;
pub(super) const ID_SIDEBAR_HEADER: usize = 2010;
pub(super) const ID_OUTLINE: usize = 2011;
pub(super) const ID_TREE: usize = 2012;
pub(super) const ID_RECENT: usize = 2013;
pub(super) const ID_SPLITTER: usize = 2014;
pub(super) const ID_DIAGNOSTIC_TEXT: usize = 2020;
pub(super) const ID_DIAGNOSTIC_CLOSE: usize = 2021;
pub(super) const ID_DIAGNOSTIC_ICON: usize = 2022;
pub(super) const ID_OUTLINE_LABEL: usize = 2015;
pub(super) const ID_TREE_LABEL: usize = 2016;
pub(super) const ID_RECENT_LABEL: usize = 2017;
pub(super) const MENU_OPEN_FILE: usize = 2101;
pub(super) const MENU_OPEN_FOLDER: usize = 2102;
pub(super) const MENU_CLOSE_TAB: usize = 2103;
pub(super) const MENU_TOGGLE_SIDEBAR: usize = 2111;
pub(super) const MENU_RELOAD: usize = 2112;
/// input: View > Find in document; handled by `input`.
pub(super) const MENU_FIND: usize = 2113;
/// View > Render text diagrams (Rust edition only, TDG-8).
pub(super) const MENU_TEXT_DIAGRAMS: usize = 2114;
pub(super) const MENU_REMOTE_ASSETS: usize = 2121;
pub(super) const MENU_HELP: usize = 2131;
pub(super) const MENU_ABOUT: usize = 2132;
pub(super) const MENU_TROUBLESHOOTING: usize = 2133;
pub(super) const MENU_SBOM: usize = 2134;
pub(super) const MENU_CHECK_UPDATES: usize = 2135;
/// The row buttons (CHROME-3): "Copy heading anchor" for the selected
/// outline row, "Reveal in File Explorer" for the selected tree row.
pub(super) const ID_COPY_ANCHOR: usize = 2018;
pub(super) const ID_REVEAL: usize = 2019;
/// The brand texts (.NET `BrandDetails`). 2030 to 2033 belong to the
/// WebView2 runtime state (`runtime`). The brand image (.NET `BrandIcon`)
/// has no window: `toolbar::paint_brand` paints it.
pub(super) const ID_BRAND_NAME: usize = 2041;
pub(super) const ID_BRAND_TAGLINE: usize = 2042;
/// `ShellControls::sidebar`, the view selector.
const ID_SELECTOR: usize = 1003;
/// `ShellControls::find` (the find box) and `ShellControls::status`.
const ID_FIND_EDIT: usize = 1004;
const ID_STATUS: usize = 1005;
/// The find bar's count text (`find::native::COUNT_ID`).
const ID_FIND_COUNT: usize = 1010;

const CBN_SELCHANGE: usize = 1;
const LBN_SELCHANGE: usize = 1;
const LBN_DBLCLK: usize = 2;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014E;
const TCM_GETCURSEL: u32 = 0x130B;
const TCN_SELCHANGE: u32 = 0xFFFF_FDD9;
const TVN_ITEMEXPANDINGW: u32 = 0xFFFF_FE3A;
const NM_CLICK: u32 = 0xFFFF_FFFE;
const NM_RETURN: u32 = 0xFFFF_FFFC;
const TVE_EXPAND: u32 = 0x2;
const TVM_SETBKCOLOR: u32 = 0x111D;
const TVM_SETTEXTCOLOR: u32 = 0x111E;
const WM_SETFONT: u32 = 0x0030;
const REPOSITORY_LOAD_FAILED: &str = "The repository tree could not be loaded.";

#[derive(Clone, Copy)]
pub(super) struct SidebarControls {
    pub header: HWND,
    /// Zero-size labels that name the lists for UIA (see `sidebar::create`).
    pub outline_label: HWND,
    pub tree_label: HWND,
    pub recent_label: HWND,
    pub outline: HWND,
    pub tree: HWND,
    pub recent: HWND,
    /// "Copy heading anchor" (outline view) and "Reveal in File Explorer"
    /// (repository view), at the right of the heading row.
    pub copy_anchor: HWND,
    pub reveal: HWND,
    pub splitter: HWND,
}

#[derive(Clone, Copy)]
pub(super) struct ChromeControls {
    pub toolbar: Toolbar,
    pub sidebar: SidebarControls,
    pub diagnostics: DiagnosticsControls,
    pub menu: Option<HMENU>,
    /// The tooltip window for the toolbar and sidebar buttons.
    pub tooltip: Option<HWND>,
}

/// DPI-scaled GDI objects for chrome controls, deleted on drop: the
/// system UI face (Segoe UI Variable Text, else Segoe UI) at the WinUI
/// sizes, and the icon font.
pub(super) struct Fonts {
    dpi: u32,
    /// Body text, 14 DIPs (toolbar labels, lists, tabs, find box).
    pub text: HFONT,
    /// Captions, 12 DIPs (status, find count, recent-file folders).
    pub caption: HFONT,
    /// Section headings, 12 DIPs SemiBold ("Document Map").
    pub heading: HFONT,
    /// Icons, 16 DIPs.
    pub glyph: HFONT,
    /// .NET `BrandDetails`: the name and the tagline.
    brand_name: HFONT,
    brand_tagline: HFONT,
}

impl Fonts {
    fn new(dpi: u32) -> Self {
        let (brand_name, brand_tagline) = toolbar::brand_fonts(dpi);
        Self {
            dpi,
            text: toolbar::text_font(dpi),
            caption: toolbar::caption_font(dpi),
            heading: toolbar::heading_font(dpi),
            glyph: toolbar::glyph_font(dpi),
            brand_name,
            brand_tagline,
        }
    }

    fn handles(&self) -> FontSet {
        FontSet {
            text: self.text,
            caption: self.caption,
            heading: self.heading,
            brand_name: self.brand_name,
            brand_tagline: self.brand_tagline,
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [
            self.text,
            self.caption,
            self.heading,
            self.glyph,
            self.brand_name,
            self.brand_tagline,
        ] {
            if !font.is_invalid() {
                // SAFETY: each font was created by this struct and is no
                // longer selected: controls get the replacement first.
                let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
            }
        }
    }
}

/// Font handles copied out of the state for `WM_SETFONT`.
#[derive(Clone, Copy)]
struct FontSet {
    text: HFONT,
    caption: HFONT,
    heading: HFONT,
    brand_name: HFONT,
    brand_tagline: HFONT,
}

enum ChromeAction {
    Command(ChromeCommand),
    SetMode(SidebarMode),
    OutlineJump(usize),
    OpenRecent(usize),
    Open(PathBuf),
    DismissDiagnostic,
    /// The "Copy heading anchor" button (.NET `CopyAnchor_Click`).
    CopyAnchor,
    /// The "Reveal in File Explorer" button (.NET `RevealFile_Click`).
    Reveal,
    /// A worker gathered the troubleshooting facts: show the dialog.
    ShowTroubleshooting(DiagnosticsFacts),
    /// A worker read the SBOM: show the dialog or the failure.
    ShowSbom(Result<(String, Vec<String>), shell_ops::SbomFailure>),
    /// An update check or install finished (`updater`).
    Update(updater::UpdateResult),
    Sync,
}

// A troubleshooting or software components dialog is open (.NET
// `_isSelfHelpDialogOpen`, `_isSbomDialogOpen`): a second request is
// ignored. Kept outside the state, so the check never borrows it.
thread_local! {
    static HELP_DIALOG_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// File-system work done off the UI thread, so an offline UNC path never
/// blocks the window. Every path passed the lexical gate first.
enum WorkerResult {
    Tree {
        generation: u64,
        result: Result<TreeArena, ScanError>,
    },
    DetectedRoot(Option<PathBuf>),
    /// The visible recent files (gated, existing), for one listing request.
    Recent {
        generation: u64,
        visible: Vec<String>,
    },
    /// A document to open from the sidebar, after its existence check.
    OpenChecked {
        path: PathBuf,
        exists: bool,
    },
    /// A folder to open as the repository, with its README if one exists.
    FolderChecked {
        folder: PathBuf,
        exists: bool,
        readme: Option<PathBuf>,
    },
    /// The saved repository root at startup.
    RootChecked {
        root: PathBuf,
        exists: bool,
    },
    /// The facts for the troubleshooting dialog (the SBOM check touches
    /// the file system).
    Troubleshooting(DiagnosticsFacts),
    /// The SBOM text and its component list, for the dialog.
    Sbom(Result<(String, Vec<String>), shell_ops::SbomFailure>),
    /// An update check or install finished (`updater`).
    Update(updater::UpdateResult),
    /// File Explorer opened, or the error message.
    Revealed(Result<(), String>),
    /// The data folder opened, or the error message.
    DataFolderOpened(Result<(), String>),
}

// Chrome actions are queued outside `AppState`, so queueing never needs a
// `RefCell` borrow and a busy borrow can never strand the queue.
thread_local! {
    static ACTIONS: RefCell<VecDeque<ChromeAction>> = const { RefCell::new(VecDeque::new()) };
    static ACTION_POSTED: Cell<bool> = const { Cell::new(false) };
    static ACTION_WINDOW: Cell<isize> = const { Cell::new(0) };
}

/// Queues `action` and posts `WM_CHROME_ACTION` once per batch.
fn queue_action(action: ChromeAction) {
    ACTIONS.with(|actions| actions.borrow_mut().push_back(action));
    let window = ACTION_WINDOW.with(Cell::get);
    if window != 0 && !ACTION_POSTED.with(Cell::get) {
        ACTION_POSTED.with(|posted| posted.set(true));
        // SAFETY: posts to this thread's own window; no pointers.
        let posted = unsafe {
            PostMessageW(
                Some(HWND(window as *mut c_void)),
                WM_CHROME_ACTION,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            ACTION_POSTED.with(|flag| flag.set(false));
        }
    }
}

fn next_action() -> Option<ChromeAction> {
    ACTIONS.with(|actions| actions.borrow_mut().pop_front())
}

/// How a worker thread reports back: the result channel, the shell window,
/// and a flag cleared when the window is destroyed.
#[derive(Clone)]
struct WorkerLink {
    window: isize,
    alive: Arc<AtomicBool>,
    sender: mpsc::Sender<WorkerResult>,
}

impl WorkerLink {
    fn deliver(&self, result: WorkerResult) {
        if !self.alive.load(Ordering::Acquire) || self.sender.send(result).is_err() {
            return;
        }
        let window = HWND(self.window as *mut c_void);
        let mut owner = 0;
        // SAFETY: plain window queries and a post without pointers. The
        // alive flag is cleared at WM_DESTROY; IsWindow and the owning
        // process guard against a recycled handle.
        unsafe {
            if !self.alive.load(Ordering::Acquire) || !IsWindow(Some(window)).as_bool() {
                return;
            }
            GetWindowThreadProcessId(window, Some(&mut owner));
            if owner != std::process::id() {
                return;
            }
            let _ = PostMessageW(Some(window), WM_CHROME_WORKER, WPARAM(0), LPARAM(0));
        }
    }

    /// Runs `work` on a named thread; `false` when the thread could not
    /// start.
    fn spawn(
        &self,
        name: &str,
        work: impl FnOnce() -> Option<WorkerResult> + Send + 'static,
    ) -> bool {
        let link = self.clone();
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                if let Some(result) = work() {
                    link.deliver(result);
                }
            })
            .is_ok()
    }
}

/// Chrome state inside the shell's `AppState`.
pub(super) struct ChromeState {
    link: WorkerLink,
    worker_results: mpsc::Receiver<WorkerResult>,
    controls: Option<ChromeControls>,
    store: Option<SettingsStore>,
    allow_remote_assets: bool,
    /// The "Render text diagrams" setting (TDG-8).
    render_text_diagrams: bool,
    mode: SidebarMode,
    sidebar_width: f64,
    dragging: bool,
    diagnostics: DiagnosticsBar,
    /// Whether the last layout had the diagnostics bar open.
    diagnostics_laid_out: bool,
    /// The last message announced to assistive technology.
    announced: Option<String>,
    outlines: BTreeMap<u64, (u64, Vec<OutlineEntry>)>,
    outline_generation: u64,
    shown_outline: Option<(u64, u64)>,
    locations: BTreeMap<u64, TabLocation>,
    /// Tabs whose first render of their path has not succeeded yet; the
    /// path joins the recent files only after it does.
    recent_pending: BTreeSet<u64>,
    reloading: BTreeSet<u64>,
    pending_scroll: BTreeSet<u64>,
    repository_root: Option<PathBuf>,
    /// The saved root, restored after startup when no document was given.
    saved_root: Option<PathBuf>,
    detecting_root: bool,
    root_cancel: Option<Arc<AtomicBool>>,
    tree: TreeArena,
    tree_populated: BTreeSet<usize>,
    tree_dirty: bool,
    tree_generation: u64,
    /// The generation of the tree shown in the control (item lParams).
    tree_shown: u32,
    tree_cancel: Option<Arc<AtomicBool>>,
    recent_dirty: bool,
    recent_generation: u64,
    recent_fill: bool,
    shown_recent: Vec<String>,
    fonts: Option<Fonts>,
    /// Dynamic MSAA/UIA annotations (accessible names and roles).
    names: Option<IAccPropServices>,
    /// The last layout was below the 720-DIP breakpoint (.NET
    /// `CompactWindow`); `None` before the first layout after `install`.
    compact: Option<bool>,
    /// The command bar width in pixels at the last layout (the theme
    /// field's "Theme: " prefix depends on it).
    bar_width: u32,
}

impl ChromeState {
    pub(super) fn new() -> Self {
        let (sender, worker_results) = mpsc::channel();
        Self {
            link: WorkerLink {
                window: 0,
                alive: Arc::new(AtomicBool::new(true)),
                sender,
            },
            worker_results,
            controls: None,
            store: None,
            allow_remote_assets: false,
            render_text_diagrams: true,
            mode: SidebarMode::Outline,
            sidebar_width: clamp_sidebar_width(f64::NAN),
            dragging: false,
            diagnostics: DiagnosticsBar::default(),
            diagnostics_laid_out: false,
            announced: None,
            outlines: BTreeMap::new(),
            outline_generation: 0,
            shown_outline: None,
            locations: BTreeMap::new(),
            recent_pending: BTreeSet::new(),
            reloading: BTreeSet::new(),
            pending_scroll: BTreeSet::new(),
            repository_root: None,
            saved_root: None,
            detecting_root: false,
            root_cancel: None,
            tree: TreeArena::default(),
            tree_populated: BTreeSet::new(),
            tree_dirty: false,
            tree_generation: 0,
            tree_shown: 0,
            tree_cancel: None,
            recent_dirty: true,
            recent_generation: 0,
            recent_fill: false,
            shown_recent: Vec::new(),
            fonts: None,
            names: None,
            compact: None,
            bar_width: 0,
        }
    }

    /// input: the open tab showing `path` (ordinal, case-insensitive), for
    /// .NET's "already open" check in every open path.
    pub(super) fn tab_for_path(&self, path: &Path) -> Option<u64> {
        self.locations
            .iter()
            .find(|(_, location)| same_document_path(location.path(), path))
            .map(|(tab, _)| *tab)
    }

    /// The (icon, text) fonts for owner-drawn buttons and the (text,
    /// caption) fonts for the tab strip; `None` before `install`.
    pub(super) fn button_fonts(&self) -> Option<(HFONT, HFONT)> {
        self.fonts.as_ref().map(|fonts| (fonts.glyph, fonts.text))
    }

    pub(super) fn text_fonts(&self) -> Option<(HFONT, HFONT)> {
        self.fonts.as_ref().map(|fonts| (fonts.text, fonts.caption))
    }

    /// Whether `tab` shows a document (not a startup placeholder).
    pub(super) fn has_document(&self, tab: u64) -> bool {
        self.locations.contains_key(&tab)
    }

    /// The shell window, once `install` ran.
    pub(super) fn window(&self) -> Option<HWND> {
        (self.link.window != 0).then_some(HWND(self.link.window as *mut c_void))
    }

    /// .NET `RenderContext.AllowRemoteAssets` for new renders.
    pub(super) fn allow_remote_assets(&self) -> bool {
        self.allow_remote_assets
    }

    /// `RenderContext::enable_text_diagrams` for new renders (TDG-8).
    pub(super) fn render_text_diagrams(&self) -> bool {
        self.render_text_diagrams
    }

    /// Sidebar width and diagnostics state for `ShellLayout`. The compact
    /// layout uses the .NET `CompactWindow` pane length (240 DIPs).
    pub(super) fn layout_inputs(&self) -> (Dip, bool) {
        let width = if self.compact == Some(true) {
            Dip::COMPACT_SIDEBAR_WIDTH
        } else {
            Dip(sidebar_width_dip(self.sidebar_width))
        };
        (width, self.diagnostics.is_open())
    }

    /// Records the document a render is for. The first render of a path in
    /// a tab is an open: once it succeeds, the path joins the recent files
    /// (`filter_render_outcome`); without a repository, the document's
    /// repository root is looked for off the UI thread. Data only; no
    /// control is touched, because `start_render` holds the state borrow.
    pub(super) fn note_render(&mut self, tab_id: u64, path: &Path) {
        if self
            .locations
            .get(&tab_id)
            .is_some_and(|location| same_document_path(location.path(), path))
        {
            return;
        }
        self.locations
            .insert(tab_id, TabLocation::new(path.to_path_buf()));
        self.recent_pending.insert(tab_id);
        if self.repository_root.is_none() && !self.detecting_root {
            self.spawn_root_detection(path.to_string_lossy().into_owned());
        }
        self.post(ChromeAction::Sync);
    }

    /// Stops repository work and worker notifications when the window
    /// closes (WM_DESTROY).
    pub(super) fn shutdown(&mut self) {
        self.link.alive.store(false, Ordering::Release);
        toolbar::clear_brand();
        // Annotations are cleared while the controls still exist; oleacc
        // keeps them per HWND otherwise.
        if let (Some(names), Some(controls)) = (self.names.take(), self.controls) {
            for window in all_windows(&controls) {
                // SAFETY: plain COM call on the UI thread; no callback into
                // the shell.
                let _ = unsafe {
                    names.ClearHwndProps(
                        window,
                        OBJID_CLIENT.0 as u32,
                        CHILDID_SELF,
                        &[
                            PROPID_ACC_NAME,
                            Name_Property_GUID,
                            IsControlElement_Property_GUID,
                            IsContentElement_Property_GUID,
                        ],
                    )
                };
            }
        }
        for cancel in [self.tree_cancel.take(), self.root_cancel.take()]
            .into_iter()
            .flatten()
        {
            cancel.store(true, Ordering::Release);
        }
        ACTION_WINDOW.with(|window| window.set(0));
    }

    fn post(&mut self, action: ChromeAction) {
        queue_action(action);
    }

    pub(super) fn diagnostic(&mut self, message: impl Into<String>, severity: DiagnosticSeverity) {
        self.diagnostics.show(message, severity);
        self.post(ChromeAction::Sync);
    }

    fn save(&mut self, change: impl FnOnce(&mut marknexia_core::settings::AppSettings)) {
        if let Some(error) = settings_bridge::update(self.store.as_mut(), change) {
            self.diagnostic(error, DiagnosticSeverity::Warning);
        }
    }

    fn add_recent(&mut self, path: &Path) {
        let text = path.to_string_lossy().into_owned();
        if let Some(error) = settings_bridge::add_recent_file(self.store.as_mut(), &text) {
            self.diagnostics.show(error, DiagnosticSeverity::Warning);
        }
        self.recent_dirty = true;
        self.post(ChromeAction::Sync);
    }

    /// .NET `DetectRepositoryRoot` for the first document without a
    /// repository. Cancelled when a repository is opened or the window
    /// closes; the result is then dropped.
    fn spawn_root_detection(&mut self, document: String) {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let started = self.link.spawn("marknexia-repository-root", move || {
            // `detect_repository_root` gates the path before any access.
            // Errors (other than access denied) mean no root.
            let root = detect_repository_root(&document).ok().flatten();
            (!worker_cancel.load(Ordering::Acquire)).then_some(WorkerResult::DetectedRoot(root))
        });
        self.detecting_root = started;
        self.root_cancel = started.then_some(cancel);
    }

    fn cancel_root_detection(&mut self) {
        if let Some(cancel) = self.root_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.detecting_root = false;
    }

    /// Starts a repository scan (.NET `RefreshRepositoryFilesAsync`): any
    /// earlier scan is cancelled and its result discarded.
    fn start_tree(&mut self, root: &Path) {
        if let Some(cancel) = self.tree_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.tree_generation = self.tree_generation.wrapping_add(1);
        let generation = self.tree_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.tree_cancel = Some(Arc::clone(&cancel));
        let root = root.to_string_lossy().into_owned();
        // `build_repository_tree` gates the root before any access.
        let started = self.link.spawn("marknexia-repository", move || {
            let result = build_repository_tree(&root, &ScanLimits::default(), &|| {
                cancel.load(Ordering::Acquire)
            })
            .map(TreeArena::from_tree);
            Some(WorkerResult::Tree { generation, result })
        });
        if !started {
            self.diagnostic(REPOSITORY_LOAD_FAILED, DiagnosticSeverity::Warning);
        }
    }

    /// Lists the recent files that pass the document path gate and exist,
    /// off the UI thread (.NET `RefreshRecentFiles`).
    fn start_recent_listing(&mut self) {
        self.recent_dirty = false;
        self.recent_generation = self.recent_generation.wrapping_add(1);
        let generation = self.recent_generation;
        let recent = settings_bridge::current(self.store.as_ref()).recent_files;
        self.link.spawn("marknexia-recent-files", move || {
            let settings = marknexia_core::settings::AppSettings {
                recent_files: recent,
                ..Default::default()
            };
            let visible = settings
                .visible_recent_files(|path| {
                    check_document_path(path).is_ok() && Path::new(path).is_file()
                })
                .into_iter()
                .map(str::to_owned)
                .collect();
            Some(WorkerResult::Recent {
                generation,
                visible,
            })
        });
    }

    /// runtime: every tab's document path, to render again once WebView2
    /// is installed.
    pub(super) fn document_tabs(&self) -> Vec<(u64, PathBuf)> {
        self.locations
            .iter()
            .map(|(tab, location)| (*tab, location.path().to_path_buf()))
            .collect()
    }

    fn active_location(&self, active: Option<u64>) -> Option<&TabLocation> {
        active.and_then(|tab| self.locations.get(&tab))
    }
}

// ---------------------------------------------------------------------------
// Small Win32 helpers shared by the chrome modules.
// ---------------------------------------------------------------------------

pub(super) fn send(window: HWND, message: u32, wparam: usize, lparam: isize) -> LRESULT {
    // SAFETY: synchronous message to a control on this thread. Callers hold
    // no AppState borrow; pointer arguments point to live locals.
    unsafe { SendMessageW(window, message, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }
}

pub(super) fn dip(value: u32, dpi: u32) -> u32 {
    Dip(value).to_pixels(dpi)
}

pub(super) fn set_bounds(window: HWND, rect: PixelRect) {
    move_child(window, rect);
}

pub(super) fn show(window: HWND, visible: bool) {
    // SAFETY: shows or hides a child window of this thread.
    let _ = unsafe { ShowWindow(window, if visible { SW_SHOWNA } else { SW_HIDE }) };
}

pub(super) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn set_text(window: HWND, text: &str) {
    let text = wide(text);
    // SAFETY: NUL-terminated buffer that outlives the call; no borrow held.
    let _ = unsafe { SetWindowTextW(window, PCWSTR::from_raw(text.as_ptr())) };
}

fn key_down(key: u16) -> bool {
    // SAFETY: reads this thread's keyboard state.
    let state = unsafe { GetKeyState(i32::from(key)) };
    state < 0
}

/// Whether a non-modifier key is down: a list selection that changed
/// meanwhile came from the keyboard (arrows, paging, type-ahead), not from
/// a click. Mouse buttons and Shift, Ctrl and Alt are ignored.
fn keyboard_input_active() -> bool {
    (0x08_u16..=0xFE).any(|key| !matches!(key, 0x10..=0x12 | 0xA0..=0xA5) && key_down(key))
}

fn state_of(hwnd: HWND) -> Option<Rc<RefCell<AppState>>> {
    // SAFETY: `hwnd` is this thread's shell window; the helper returns a
    // scoped strong reference and no borrow.
    unsafe { app_state_handle(hwnd) }
}

fn to_stored(theme: ThemePreference) -> StoredTheme {
    match theme {
        ThemePreference::System => StoredTheme::System,
        ThemePreference::Light => StoredTheme::Light,
        ThemePreference::Dark => StoredTheme::Dark,
    }
}

fn from_stored(theme: StoredTheme) -> ThemePreference {
    match theme {
        StoredTheme::System => ThemePreference::System,
        StoredTheme::Light => ThemePreference::Light,
        StoredTheme::Dark => ThemePreference::Dark,
    }
}

fn theme_index(theme: ThemePreference) -> usize {
    match theme {
        ThemePreference::System => 0,
        ThemePreference::Light => 1,
        ThemePreference::Dark => 2,
    }
}

fn post_action(_hwnd: HWND, action: ChromeAction) {
    queue_action(action);
}

// ---------------------------------------------------------------------------
// Hooks called from window.rs.
// ---------------------------------------------------------------------------

/// Creates the chrome controls and the menu bar and applies the saved
/// settings (.NET `MainWindow` constructor: `ApplySavedTheme`, pane state,
/// sidebar mode, remote assets). Runs before startup documents render, so
/// they use the saved theme and remote-image policy.
pub(super) fn install(
    hwnd: HWND,
    instance: HINSTANCE,
    state: &Rc<RefCell<AppState>>,
) -> Result<(), WindowError> {
    sidebar::register_splitter(instance)?;
    let toolbar = toolbar::create(hwnd, instance)?;
    let sidebar_controls = sidebar::create(hwnd, instance)?;
    let diagnostics_controls = diagnostics::create(hwnd, instance)?;
    let menu = toolbar::create_menu(hwnd, updater::menu_enabled());
    // .NET `ToolTipService.ToolTip` texts; a missing tooltip window only
    // loses the tooltips.
    let tooltip = toolbar::create_tooltips(
        hwnd,
        instance,
        &toolbar,
        &[
            (sidebar_controls.splitter, "Drag to resize sidebar"),
            (sidebar_controls.copy_anchor, "Copy anchor"),
            (sidebar_controls.reveal, "Reveal in File Explorer"),
        ],
    );
    let (store, notice) = match settings_bridge::open() {
        Some((store, notice)) => (Some(store), notice),
        None => (None, None),
    };
    let settings = settings_bridge::current(store.as_ref());
    let (portable, shell, dpi) = {
        let state = state.borrow();
        (Rc::clone(&state.portable), state.controls, state.dpi)
    };
    {
        let mut app = portable.borrow_mut();
        app.set_theme(from_stored(settings.theme));
        app.set_sidebar_visible(settings.is_sidebar_open);
    }
    let mode = SidebarMode::from_setting(settings.sidebar_mode);
    let fonts = Fonts::new(dpi);
    let controls = ChromeControls {
        toolbar,
        sidebar: sidebar_controls,
        diagnostics: diagnostics_controls,
        menu,
        tooltip,
    };
    if let Some(shell) = shell {
        apply_fonts(&controls, shell, None, fonts.handles());
        order_controls(&controls, shell);
        sidebar::select_mode(shell.sidebar, mode);
        // Pointer hover for the owner-drawn buttons and lists, and the flat
        // pivot of the view selector.
        for button in [
            shell.command,
            controls.sidebar.copy_anchor,
            controls.sidebar.reveal,
            controls.diagnostics.close,
        ] {
            style::track_hover(button, style::HoverKind::Button);
        }
        for list in [controls.sidebar.outline, controls.sidebar.recent] {
            style::track_hover(list, style::HoverKind::List);
        }
        sidebar::flatten_selector(shell.sidebar);
    }
    {
        let mut state = state.borrow_mut();
        let chrome = &mut state.chrome;
        chrome.link.window = hwnd.0 as isize;
        ACTION_WINDOW.with(|window| window.set(hwnd.0 as isize));
        chrome.allow_remote_assets = settings.allow_remote_assets;
        chrome.render_text_diagrams = settings.render_text_diagrams;
        chrome.mode = mode;
        chrome.sidebar_width = clamp_sidebar_width(settings.sidebar_width);
        // Restored in `after_startup`; until then a startup document may
        // set the root from its own repository, as in .NET.
        chrome.saved_root = settings.repository_root.as_ref().map(PathBuf::from);
        chrome.store = store;
        chrome.fonts = Some(fonts);
        chrome.controls = Some(controls);
        if let Some(notice) = notice {
            chrome
                .diagnostics
                .show(notice.to_string(), DiagnosticSeverity::Warning);
        }
    }
    Ok(())
}

/// After startup documents were queued: without them, restore the saved
/// repository (.NET `RestoreSavedRepositoryRootAsync`); then show state.
pub(super) fn after_startup(hwnd: HWND, state: &Rc<RefCell<AppState>>, document_mode: bool) {
    annotate_controls(state);
    // The find bar exists now (`input::initialize` runs after `install`).
    let fonts = state
        .try_borrow()
        .ok()
        .and_then(|state| state.chrome.fonts.as_ref().map(Fonts::handles));
    if let (Some(find), Some(fonts)) = (super::input::find_windows(state), fonts) {
        send(find.label, WM_SETFONT, fonts.text.0 as usize, 1);
        send(find.count, WM_SETFONT, fonts.caption.0 as usize, 1);
        for button in [find.previous, find.next, find.close] {
            style::track_hover(button, style::HoverKind::Button);
        }
    }
    {
        let mut state = state.borrow_mut();
        let chrome = &mut state.chrome;
        // With a startup document .NET resolves the root from that document
        // instead (see `note_render`).
        if let (false, Some(root)) = (document_mode, chrome.saved_root.take()) {
            if GatedPath::new(&root.to_string_lossy()).is_ok() {
                // The existence check can block on an offline share.
                let started = chrome.link.spawn("marknexia-repository-restore", move || {
                    let exists = root.is_dir();
                    Some(WorkerResult::RootChecked { root, exists })
                });
                if !started {
                    chrome.diagnostic(REPOSITORY_LOAD_FAILED, DiagnosticSeverity::Warning);
                }
            } else {
                chrome.save(|settings| settings.repository_root = None);
            }
        }
    }
    sync_ui(hwnd);
}

/// The chrome's window messages; `None` leaves the message to window.rs.
pub(super) fn handle_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    match message {
        WM_COMMAND => on_command(hwnd, wparam, lparam),
        WM_NOTIFY => on_notify(hwnd, lparam),
        // SAFETY: for WM_DRAWITEM, lParam points to a DRAWITEMSTRUCT that is
        // valid for the duration of this message.
        WM_DRAWITEM => unsafe { (lparam.0 as *const DRAWITEMSTRUCT).as_ref() }
            .and_then(|item| on_draw_item(hwnd, item)),
        // SAFETY: for WM_MEASUREITEM, lParam points to a MEASUREITEMSTRUCT
        // that is valid and writable for the duration of this message.
        WM_MEASUREITEM => unsafe { (lparam.0 as *mut MEASUREITEMSTRUCT).as_mut() }
            .is_some_and(|item| measure_theme(hwnd, item) || menu_theme::measure(hwnd, item))
            .then_some(LRESULT(1)),
        WM_MENUCHAR => menu_theme::menu_char(menu_handle(hwnd), wparam, lparam),
        WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX | WM_CTLCOLOREDIT => {
            control_color(hwnd, message, HDC(wparam.0 as *mut c_void), lparam)
        }
        // The brand image (no child window); reads no shell state.
        WM_PAINT => {
            toolbar::paint_brand(hwnd);
            Some(LRESULT(0))
        }
        // Dispatched by a nested pump while an outer frame holds the state:
        // run from the message loop instead (input's deferred work).
        WM_CHROME_ACTION | WM_CHROME_WORKER
            if state_of(hwnd).is_some_and(|state| !super::shell_free(&state)) =>
        {
            super::input::defer_chrome(message == WM_CHROME_WORKER);
            Some(LRESULT(0))
        }
        WM_CHROME_ACTION => {
            run_actions(hwnd);
            Some(LRESULT(0))
        }
        WM_CHROME_WORKER => {
            apply_worker_results(hwnd);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// The shell's menu bar, read from the window (no state borrow).
fn menu_handle(hwnd: HWND) -> Option<HMENU> {
    // SAFETY: plain query of this thread's window.
    let menu = unsafe { windows::Win32::UI::WindowsAndMessaging::GetMenu(hwnd) };
    (!menu.is_invalid()).then_some(menu)
}

/// Keyboard handling from the message loop, before dispatch: the .NET
/// accelerators, Tab traversal between chrome controls, and Enter/Space on
/// the outline and recent lists. Runs with no borrow held.
pub(super) fn translate_message(hwnd: HWND, msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return false;
    }
    // SAFETY: plain window relationship query.
    if msg.hwnd != hwnd && !unsafe { IsChild(hwnd, msg.hwnd) }.as_bool() {
        return false;
    }
    let key = msg.wParam.0 as u16;
    let ctrl = key_down(VK_CONTROL.0);
    let shift = key_down(VK_SHIFT.0);
    let alt = msg.message == WM_SYSKEYDOWN || key_down(VK_MENU.0);
    if let Some(command) = route_chrome_key(key, ctrl, shift, alt) {
        run_command(hwnd, command);
        return true;
    }
    if msg.message != WM_KEYDOWN || ctrl || alt {
        return false;
    }
    if key == VK_TAB.0 {
        return move_focus(hwnd, shift);
    }
    if key == VK_RETURN.0 || key == VK_SPACE.0 {
        let lists = state_of(hwnd).and_then(|state| {
            state.try_borrow().ok().and_then(|state| {
                state
                    .chrome
                    .controls
                    .as_ref()
                    .map(|c| (c.sidebar.outline, c.sidebar.recent))
            })
        });
        // SAFETY: reads this thread's focus window.
        let focus = unsafe { GetFocus() };
        if let Some((outline, recent)) = lists {
            if focus == outline {
                if let Some(index) = sidebar::list_selection(outline) {
                    run_action(hwnd, ChromeAction::OutlineJump(index));
                }
                return true;
            }
            if focus == recent {
                if let Some(index) = sidebar::list_selection(recent) {
                    run_action(hwnd, ChromeAction::OpenRecent(index));
                }
                return true;
            }
        }
    }
    false
}

/// A shell accelerator pressed inside the document that `route_key` did
/// not handle.
pub(super) fn route_accelerator(hwnd: HWND, chord: ShellAccelerator) {
    if let Some(command) = route_chrome_key(chord.virtual_key, chord.ctrl, chord.shift, chord.alt) {
        run_command(hwnd, command);
    }
}

/// Selection or tab set changed: refresh the outline and history buttons.
pub(super) fn tabs_changed(_state: &Rc<RefCell<AppState>>) {
    // The action queue lives outside the state: this never borrows it, so
    // a refresh from a reentrant path is not lost.
    queue_action(ChromeAction::Sync);
}

/// Applies a render result's chrome effects before window.rs hosts it.
/// Returns `None` when the result must not replace the tab's document: a
/// failed reload keeps the previous document, as .NET `Reload_Click` does.
pub(super) fn filter_render_outcome(
    state: &Rc<RefCell<AppState>>,
    tab_id: u64,
    result: Result<marknexia_webview::policy::HostDocument, OpenError>,
    outline: Vec<OutlineEntry>,
) -> Option<Result<marknexia_webview::policy::HostDocument, OpenError>> {
    let mut state = state.borrow_mut();
    let chrome = &mut state.chrome;
    let reloading = chrome.reloading.remove(&tab_id);
    match &result {
        Ok(_) => {
            chrome.outline_generation = chrome.outline_generation.wrapping_add(1);
            chrome
                .outlines
                .insert(tab_id, (chrome.outline_generation, outline));
            if reloading {
                // .NET `PendingScroll`: restore the location after the load.
                chrome.pending_scroll.insert(tab_id);
            }
            if chrome.recent_pending.remove(&tab_id)
                && let Some(path) = chrome
                    .locations
                    .get(&tab_id)
                    .map(|location| location.path().to_path_buf())
            {
                chrome.add_recent(&path);
            }
            chrome.post(ChromeAction::Sync);
            Some(result)
        }
        Err(error) if reloading => {
            chrome.diagnostic(
                format!("Unable to reload document: {error}"),
                DiagnosticSeverity::Error,
            );
            None
        }
        Err(error) => {
            chrome.recent_pending.remove(&tab_id);
            chrome.outlines.remove(&tab_id);
            chrome.diagnostic(
                format!("Unable to open document: {error}"),
                DiagnosticSeverity::Error,
            );
            Some(result)
        }
    }
}

/// Scrolls reloaded documents to their stored location once they loaded.
pub(super) fn after_poll(state: &Rc<RefCell<AppState>>) {
    let pending: Vec<(u64, Option<String>)> = {
        let Ok(state) = state.try_borrow() else {
            return;
        };
        if state.chrome.pending_scroll.is_empty() {
            return;
        }
        state
            .chrome
            .pending_scroll
            .iter()
            .map(|tab| {
                (
                    *tab,
                    state
                        .chrome
                        .locations
                        .get(tab)
                        .and_then(|location| location.fragment().map(str::to_owned)),
                )
            })
            .collect()
    };
    let Some(session) = take_session(state) else {
        return;
    };
    let mut done = Vec::new();
    for (tab, fragment) in pending {
        match session.host(tab) {
            Some(host) if host.document_loaded() => {
                if fragment.is_some() {
                    let _ = host.scroll_to_anchor(fragment.as_deref());
                }
                done.push(tab);
            }
            Some(_) => {}
            // The tab closed or lost its controller.
            None => done.push(tab),
        }
    }
    restore_session(state, session);
    let mut state = state.borrow_mut();
    for tab in done {
        state.chrome.pending_scroll.remove(&tab);
    }
}

/// `reflow`, before the layout: applies the .NET `WideWindow` and
/// `CompactWindow` visual states for a client `width` at `dpi`. Crossing
/// the 720-DIP breakpoint closes the pane (compact) or opens it (wide),
/// without saving `IsSidebarOpen`. The caller checked `is_free`.
pub(super) fn update_compact(state: &Rc<RefCell<AppState>>, width: u32, dpi: u32) {
    let Ok(mut app) = state.try_borrow_mut() else {
        return;
    };
    // Before `install` the saved pane state is not applied yet.
    if app.chrome.controls.is_none() || width == 0 {
        return;
    }
    let compact = crate::layout::is_compact(width, dpi);
    let previous = app.chrome.compact;
    let Some(visible) = compact_transition(previous, compact) else {
        app.chrome.compact = Some(compact);
        return;
    };
    // The state is recorded only once the pane follows it; otherwise the
    // next layout retries the transition.
    let portable = Rc::clone(&app.portable);
    if let Ok(mut portable) = portable.try_borrow_mut() {
        portable.set_sidebar_visible(visible);
        app.chrome.compact = Some(compact);
    }
}

/// Positions the chrome controls after window.rs placed the shell controls.
pub(super) fn layout_children(hwnd: HWND, layout: &ShellLayout) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let (shell, dpi, mode, diagnostics_open, refont, compact, theme) = {
        let Ok(state) = state.try_borrow() else {
            return;
        };
        let refont = state
            .chrome
            .fonts
            .as_ref()
            .is_none_or(|fonts| fonts.dpi != state.dpi);
        (
            state.controls,
            state.dpi,
            state.chrome.mode,
            state.chrome.diagnostics.is_open(),
            refont,
            state.chrome.compact == Some(true),
            state.theme,
        )
    };
    let Some(shell) = shell else {
        return;
    };
    let find = super::input::find_windows(&state);
    if refont {
        let fonts = Fonts::new(dpi);
        let selected = fonts.handles();
        // The new fonts are installed before the old ones are deleted.
        let previous = {
            let mut state = state.borrow_mut();
            state.chrome.fonts.replace(fonts)
        };
        with_controls(&state, |controls| {
            apply_fonts(controls, shell, find, selected);
            // The menu font follows the DPI too.
            if let Some(menu) = controls.menu {
                menu_theme::apply(hwnd, menu, theme, dpi);
            }
        });
        drop(previous);
    }
    with_controls(&state, |controls| {
        toolbar::layout(controls, shell.command, layout.command_bar, dpi, compact);
        sidebar::layout(
            shell.sidebar,
            &controls.sidebar,
            layout.sidebar,
            layout.splitter,
            mode,
            dpi,
        );
        // The compact pane has the fixed .NET length: no resize handle.
        if compact {
            show(controls.sidebar.splitter, false);
        }
        // SAFETY: repaint requests for children of this thread; the Open
        // and Folder faces follow the compact state.
        unsafe {
            let _ = InvalidateRect(Some(shell.command), None, true);
            let _ = InvalidateRect(Some(controls.toolbar.folder), None, true);
        }
        diagnostics::layout(&controls.diagnostics, layout.diagnostics, dpi);
        let open = diagnostics_open && layout.diagnostics.height > 0;
        show(controls.diagnostics.icon, open);
        show(controls.diagnostics.text, open);
        show(controls.diagnostics.close, open);
    });
    if let Ok(mut state) = state.try_borrow_mut() {
        state.chrome.diagnostics_laid_out = diagnostics_open;
        state.chrome.bar_width = layout.command_bar.width;
    }
}

/// Theme-dependent control styles (dark variants in the dark theme, system
/// defaults otherwise, so high contrast uses system colors).
pub(super) fn theme_changed(state: &Rc<RefCell<AppState>>) {
    let (theme, dpi, window) = {
        let state = state.borrow();
        (state.theme, state.dpi, ACTION_WINDOW.with(Cell::get))
    };
    let colors = palette(theme);
    with_controls(state, |controls| {
        // The dark menu bar (owner-drawn items) or the system one.
        if let Some(menu) = controls.menu
            && window != 0
        {
            menu_theme::apply(HWND(window as *mut c_void), menu, theme, dpi);
        }
        let dark = theme == EffectiveTheme::Dark;
        let tree = controls.sidebar.tree;
        let explorer = if dark {
            w!("DarkMode_Explorer")
        } else if theme == EffectiveTheme::Light {
            w!("Explorer")
        } else {
            PCWSTR::null()
        };
        // Tooltips follow the theme as the lists do.
        for window in [tree, controls.sidebar.outline, controls.sidebar.recent]
            .into_iter()
            .chain(controls.tooltip)
        {
            // SAFETY: theme names are static strings.
            let _ = unsafe { SetWindowTheme(window, explorer, PCWSTR::null()) };
        }
        // SAFETY: as above.
        let _ = unsafe {
            SetWindowTheme(
                controls.toolbar.theme,
                if dark {
                    w!("DarkMode_CFD")
                } else {
                    PCWSTR::null()
                },
                PCWSTR::null(),
            )
        };
        // The tree on the sidebar color (system colors in high contrast).
        let sidebar = style::sidebar_colors(theme, colors);
        let (background, text) = if theme == EffectiveTheme::HighContrast {
            (-1, -1)
        } else {
            (
                super::native_color(sidebar.background).0 as isize,
                super::native_color(sidebar.text).0 as isize,
            )
        };
        send(tree, TVM_SETBKCOLOR, 0, background);
        send(tree, TVM_SETTEXTCOLOR, 0, text);
        for window in all_windows(controls) {
            // SAFETY: repaint request for a child of this thread.
            let _ = unsafe { InvalidateRect(Some(window), None, true) };
        }
    });
}

// ---------------------------------------------------------------------------
// Messages.
// ---------------------------------------------------------------------------

fn on_command(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let id = wparam.0 & 0xffff;
    let code = (wparam.0 >> 16) & 0xffff;
    let clicked = code == 0;
    let action = match id {
        ID_TOGGLE_SIDEBAR | MENU_TOGGLE_SIDEBAR if clicked => {
            ChromeAction::Command(ChromeCommand::ToggleSidebar)
        }
        ID_BACK if clicked => ChromeAction::Command(ChromeCommand::Back),
        ID_FORWARD if clicked => ChromeAction::Command(ChromeCommand::Forward),
        ID_RELOAD | MENU_RELOAD if clicked => ChromeAction::Command(ChromeCommand::Reload),
        ID_OPEN_FOLDER | MENU_OPEN_FOLDER if clicked => {
            ChromeAction::Command(ChromeCommand::OpenFolder)
        }
        MENU_OPEN_FILE if clicked => ChromeAction::Command(ChromeCommand::OpenFile),
        MENU_CLOSE_TAB if clicked => ChromeAction::Command(ChromeCommand::CloseTab),
        MENU_REMOTE_ASSETS if clicked => ChromeAction::Command(ChromeCommand::ToggleRemoteAssets),
        MENU_TEXT_DIAGRAMS if clicked => ChromeAction::Command(ChromeCommand::ToggleTextDiagrams),
        MENU_HELP if clicked => ChromeAction::Command(ChromeCommand::ShowHelp),
        MENU_ABOUT if clicked => ChromeAction::Command(ChromeCommand::ShowAbout),
        MENU_TROUBLESHOOTING if clicked => {
            ChromeAction::Command(ChromeCommand::ShowTroubleshooting)
        }
        MENU_SBOM if clicked => ChromeAction::Command(ChromeCommand::ShowSbom),
        MENU_CHECK_UPDATES if clicked => ChromeAction::Command(ChromeCommand::CheckForUpdates),
        ID_COPY_ANCHOR if clicked => ChromeAction::CopyAnchor,
        ID_REVEAL if clicked => ChromeAction::Reveal,
        ID_DIAGNOSTIC_CLOSE if clicked => ChromeAction::DismissDiagnostic,
        ID_THEME if code == CBN_SELCHANGE => {
            let selected = send(HWND(lparam.0 as *mut c_void), CB_GETCURSEL, 0, 0).0;
            let theme = match selected {
                1 => ThemePreference::Light,
                2 => ThemePreference::Dark,
                _ => ThemePreference::System,
            };
            ChromeAction::Command(ChromeCommand::SetTheme(theme))
        }
        ID_OUTLINE | ID_RECENT if code == LBN_DBLCLK || code == LBN_SELCHANGE => {
            // Keyboard selection only moves focus (.NET `IsItemClickEnabled`);
            // a click or Enter opens the item.
            if code == LBN_SELCHANGE && keyboard_input_active() {
                return Some(LRESULT(0));
            }
            let Some(index) = sidebar::list_selection(HWND(lparam.0 as *mut c_void)) else {
                return Some(LRESULT(0));
            };
            if id == ID_OUTLINE {
                ChromeAction::OutlineJump(index)
            } else {
                ChromeAction::OpenRecent(index)
            }
        }
        _ => return None,
    };
    post_action(hwnd, action);
    Some(LRESULT(0))
}

fn on_notify(hwnd: HWND, lparam: LPARAM) -> Option<LRESULT> {
    // SAFETY: for WM_NOTIFY, lParam points to an NMHDR (or a larger
    // structure that starts with one) valid for this message.
    let header = unsafe { (lparam.0 as *const NMHDR).as_ref() }?;
    match (header.idFrom, header.code) {
        (ID_SELECTOR, TCN_SELCHANGE) => {
            let index = send(header.hwndFrom, TCM_GETCURSEL, 0, 0).0;
            if let Some(mode) = usize::try_from(index)
                .ok()
                .and_then(SidebarMode::from_index)
            {
                post_action(hwnd, ChromeAction::SetMode(mode));
            }
            Some(LRESULT(0))
        }
        (ID_TREE, NM_CLICK) => {
            let item = sidebar::tree_item_at_cursor(header.hwndFrom);
            open_tree_item(hwnd, header.hwndFrom, item);
            Some(LRESULT(0))
        }
        (ID_TREE, NM_RETURN) => {
            let item = sidebar::tree_selection(header.hwndFrom);
            open_tree_item(hwnd, header.hwndFrom, item);
            Some(LRESULT(1))
        }
        (ID_TREE, TVN_ITEMEXPANDINGW) => {
            // SAFETY: TVN_ITEMEXPANDINGW carries an NMTREEVIEWW.
            let view = unsafe { (lparam.0 as *const NMTREEVIEWW).as_ref() }?;
            if view.action.0 & TVE_EXPAND != 0 {
                expand_tree_item(
                    hwnd,
                    header.hwndFrom,
                    view.itemNew.hItem,
                    view.itemNew.lParam,
                );
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn open_tree_item(hwnd: HWND, tree: HWND, item: HTREEITEM) {
    let Some((generation, index)) = sidebar::tree_item_index(tree, item) else {
        return;
    };
    let path = state_of(hwnd).and_then(|state| {
        let state = state.try_borrow().ok()?;
        // An item of an earlier tree is stale.
        if generation != state.chrome.tree_shown {
            return None;
        }
        let node = state.chrome.tree.node(index)?;
        (!node.is_folder).then(|| node.path.clone())
    });
    if let Some(path) = path {
        post_action(hwnd, ChromeAction::Open(path));
    }
}

/// Fills a folder on first expansion (children are inserted lazily).
fn expand_tree_item(hwnd: HWND, tree: HWND, item: HTREEITEM, data: LPARAM) {
    let (generation, index) = sidebar::decode_tree_item(data);
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let children: Vec<(usize, TreeNode)> = {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let chrome = &mut state.chrome;
        if generation != chrome.tree_shown || !chrome.tree_populated.insert(index) {
            return;
        }
        let Some(node) = chrome.tree.node(index) else {
            return;
        };
        node.children
            .iter()
            .filter_map(|child| chrome.tree.node(*child).map(|node| (*child, node.clone())))
            .collect()
    };
    sidebar::insert_children(
        tree,
        item,
        generation,
        children
            .iter()
            .map(|(index, node)| (*index, node))
            .collect(),
    );
}

fn on_draw_item(hwnd: HWND, item: &DRAWITEMSTRUCT) -> Option<LRESULT> {
    // The dark menu bar's items (no state borrow).
    if menu_theme::draw(item) {
        return Some(LRESULT(1));
    }
    let id = item.CtlID as usize;
    if !matches!(
        id,
        ID_DIAGNOSTIC_ICON
            | ID_DIAGNOSTIC_CLOSE
            | ID_OUTLINE
            | ID_RECENT
            | ID_COPY_ANCHOR
            | ID_REVEAL
            | ID_THEME
    ) && toolbar::button_face(id, false).is_none()
    {
        return None;
    }
    // Copy what painting needs, then release the borrow: painting the
    // lists sends LB_GETTEXT to the list box.
    let (theme, colors, fonts, dpi, message, compact, bar_width) = {
        let state = state_of(hwnd)?;
        let state = state.try_borrow().ok()?;
        let fonts = state.chrome.fonts.as_ref()?;
        (
            state.theme,
            palette(state.theme),
            (fonts.glyph, fonts.text, fonts.caption),
            state.dpi,
            (id == ID_DIAGNOSTIC_ICON)
                .then(|| state.chrome.diagnostics.current().cloned())
                .flatten(),
            state.chrome.compact == Some(true),
            state.chrome.bar_width,
        )
    };
    let face = toolbar::button_face(id, compact);
    let sidebar = style::sidebar_colors(theme, colors);
    let buttons = (fonts.0, fonts.1);
    match (id, face) {
        (ID_DIAGNOSTIC_ICON, _) => {
            diagnostics::draw_icon(item, theme, colors, message.as_ref(), fonts.0, dpi);
        }
        (ID_DIAGNOSTIC_CLOSE, _) => toolbar::draw_button(
            item,
            theme,
            colors,
            diagnostics::background(theme, colors),
            '\u{E711}',
            None,
            buttons,
            dpi,
        ),
        // .NET glyphs: Copy (E8C8) and OpenInNewWindow (E8B7).
        (ID_COPY_ANCHOR, _) => toolbar::draw_button(
            item,
            theme,
            colors,
            sidebar.background,
            '\u{E8C8}',
            None,
            buttons,
            dpi,
        ),
        (ID_REVEAL, _) => toolbar::draw_button(
            item,
            theme,
            colors,
            sidebar.background,
            '\u{E8B7}',
            None,
            buttons,
            dpi,
        ),
        (ID_OUTLINE, _) => sidebar::draw_outline_item(item, sidebar, fonts.1, dpi),
        (ID_RECENT, _) => sidebar::draw_recent_item(item, sidebar, (fonts.1, fonts.2), dpi),
        (ID_THEME, _) => toolbar::draw_theme_item(
            item,
            theme,
            colors,
            fonts.1,
            dpi,
            toolbar::theme_prefix(compact, bar_width, dpi),
        ),
        (_, Some((glyph, label))) => toolbar::draw_button(
            item,
            theme,
            colors,
            colors.surface,
            glyph,
            label,
            buttons,
            dpi,
        ),
        _ => return None,
    }
    Some(LRESULT(1))
}

/// `WM_MEASUREITEM` for the owner-drawn theme selector: the field and the
/// drop-down items meet the 44-DIP target (the field height is set again
/// on every layout, with `CB_SETITEMHEIGHT`).
fn measure_theme(hwnd: HWND, item: &mut MEASUREITEMSTRUCT) -> bool {
    const ODT_COMBOBOX: u32 = 3;
    if item.CtlType.0 != ODT_COMBOBOX || item.CtlID as usize != ID_THEME {
        return false;
    }
    let dpi = state_of(hwnd)
        .and_then(|state| state.try_borrow().ok().map(|state| state.dpi))
        .unwrap_or(96);
    item.itemHeight = dip(44, dpi);
    true
}

/// `WM_CTLCOLOR*` for the chrome's text controls in every theme: the brand
/// texts on the command bar, the sidebar heading and lists on the sidebar
/// color, the status bar and the find bar's count and box. High contrast
/// uses system colors only. Other controls keep the shell's handling.
fn control_color(hwnd: HWND, message: u32, dc: HDC, lparam: LPARAM) -> Option<LRESULT> {
    let control = HWND(lparam.0 as *mut c_void);
    // SAFETY: plain query of a child window of this thread.
    let id =
        usize::try_from(unsafe { windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(control) })
            .ok()?;
    let (theme, colors) = {
        let state = state_of(hwnd)?;
        let state = state.try_borrow().ok()?;
        (state.theme, palette(state.theme))
    };
    let sidebar = style::sidebar_colors(theme, colors);
    let window_pair = (
        ColorSpec::System(SystemColorRole::Window),
        ColorSpec::System(SystemColorRole::WindowText),
    );
    let high_contrast = theme == EffectiveTheme::HighContrast;
    let (background, text) = match (message, id) {
        (WM_CTLCOLORSTATIC, ID_SIDEBAR_HEADER) => (sidebar.background, sidebar.muted),
        (WM_CTLCOLORLISTBOX, ID_OUTLINE | ID_RECENT) => (sidebar.background, sidebar.text),
        (WM_CTLCOLORSTATIC, ID_BRAND_NAME | ID_BRAND_TAGLINE | ID_STATUS | ID_FIND_COUNT)
        | (WM_CTLCOLOREDIT, ID_FIND_EDIT)
            if high_contrast =>
        {
            window_pair
        }
        (WM_CTLCOLORSTATIC, ID_BRAND_NAME) => (colors.surface, colors.text),
        (WM_CTLCOLORSTATIC, ID_BRAND_TAGLINE | ID_STATUS | ID_FIND_COUNT) => {
            (colors.surface, colors.text_secondary)
        }
        (WM_CTLCOLOREDIT, ID_FIND_EDIT) => (colors.surface, colors.text),
        _ => return None,
    };
    let (background, text) = (native_color(background), native_color(text));
    // SAFETY: the DC belongs to this WM_CTLCOLOR* message.
    unsafe {
        SetTextColor(dc, text);
        SetBkColor(dc, background);
    }
    Some(LRESULT(style::brush(background).0 as isize))
}

/// The foreground color for text on `background`: in high contrast, text on
/// a button face uses ButtonText (WindowText belongs on Window).
pub(super) fn readable_on(background: ColorSpec, foreground: ColorSpec) -> ColorSpec {
    match (background, foreground) {
        (
            ColorSpec::System(SystemColorRole::ButtonFace),
            ColorSpec::System(SystemColorRole::WindowText),
        ) => ColorSpec::System(SystemColorRole::ButtonText),
        _ => foreground,
    }
}

// ---------------------------------------------------------------------------
// Actions and commands (message loop, no borrow held).
// ---------------------------------------------------------------------------

pub(super) fn run_actions(hwnd: HWND) {
    ACTION_POSTED.with(|posted| posted.set(false));
    while let Some(action) = next_action() {
        run_action(hwnd, action);
    }
}

fn run_action(hwnd: HWND, action: ChromeAction) {
    match action {
        ChromeAction::Command(command) => run_command(hwnd, command),
        ChromeAction::SetMode(mode) => set_mode(hwnd, mode),
        ChromeAction::OutlineJump(index) => outline_jump(hwnd, index),
        ChromeAction::OpenRecent(index) => {
            let path = state_of(hwnd).and_then(|state| {
                state
                    .try_borrow()
                    .ok()
                    .and_then(|state| state.chrome.shown_recent.get(index).cloned())
            });
            if let Some(path) = path {
                open_or_select(hwnd, PathBuf::from(path));
            }
        }
        ChromeAction::Open(path) => open_or_select(hwnd, path),
        ChromeAction::DismissDiagnostic => {
            if let Some(state) = state_of(hwnd) {
                state.borrow_mut().chrome.diagnostics.dismiss();
            }
            sync_ui(hwnd);
        }
        ChromeAction::CopyAnchor => copy_anchor(hwnd),
        ChromeAction::Reveal => reveal_selected(hwnd),
        ChromeAction::ShowTroubleshooting(facts) => show_troubleshooting(hwnd, &facts),
        ChromeAction::ShowSbom(result) => show_sbom(hwnd, result),
        ChromeAction::Update(result) => updater::apply(hwnd, result),
        ChromeAction::Sync => sync_ui(hwnd),
    }
}

/// .NET `CopyAnchor_Click` for the selected outline row: the clipboard gets
/// `#slug` and nothing else; the diagnostics bar confirms it.
fn copy_anchor(hwnd: HWND) {
    let Some(state) = state_of(hwnd).filter(|state| super::shell_free(state)) else {
        return;
    };
    let Some(outline) = state
        .try_borrow()
        .ok()
        .and_then(|state| state.chrome.controls.map(|c| c.sidebar.outline))
    else {
        return;
    };
    // Control query and clipboard calls run with no borrow held.
    let Some(index) = sidebar::list_selection(outline) else {
        return;
    };
    let Some(active) = active_tab(&state) else {
        return;
    };
    let slug = state.try_borrow().ok().and_then(|state| {
        state
            .chrome
            .outlines
            .get(&active)
            .and_then(|(_, entries)| entries.get(index))
            .map(|entry| entry.slug.clone())
    });
    let Some(slug) = slug else {
        return;
    };
    let copied = shell_ops::copy_text(hwnd, &anchor_clipboard_text(&slug));
    with_chrome(hwnd, |chrome| match copied {
        Ok(()) => chrome.diagnostic(anchor_copied_notice(&slug), DiagnosticSeverity::Info),
        Err(_) => chrome.diagnostic(ANCHOR_COPY_FAILED, DiagnosticSeverity::Warning),
    });
}

/// .NET `RevealFile_Click` for the selected tree row: the path passes the
/// gate and must lie in the repository; File Explorer opens on a worker
/// with the item selected in its folder.
fn reveal_selected(hwnd: HWND) {
    let Some(state) = state_of(hwnd).filter(|state| super::shell_free(state)) else {
        return;
    };
    let Some(tree) = state
        .try_borrow()
        .ok()
        .and_then(|state| state.chrome.controls.map(|c| c.sidebar.tree))
    else {
        return;
    };
    let selected = sidebar::tree_selection(tree);
    let Some((generation, index)) = sidebar::tree_item_index(tree, selected) else {
        return;
    };
    with_chrome(hwnd, |chrome| {
        if generation != chrome.tree_shown {
            return;
        }
        let Some(path) = chrome.tree.node(index).map(|node| node.path.clone()) else {
            return;
        };
        let Some(target) = shell_ops::RevealTarget::new(&path, chrome.repository_root.as_deref())
        else {
            chrome.diagnostic(
                reveal_failed_notice("The path is not supported."),
                DiagnosticSeverity::Warning,
            );
            return;
        };
        let started = chrome.link.spawn("marknexia-reveal", move || {
            Some(WorkerResult::Revealed(shell_ops::reveal(&target)))
        });
        if !started {
            chrome.diagnostic(
                reveal_failed_notice("the request could not start."),
                DiagnosticSeverity::Warning,
            );
        }
    });
}

/// .NET `ShowSelfHelp_Click`: gathers the facts (the SBOM check on a
/// worker), then shows the dialog from `apply_worker_results`.
fn request_troubleshooting(hwnd: HWND) {
    if HELP_DIALOG_OPEN.with(Cell::get) {
        return;
    }
    with_chrome(hwnd, |chrome| {
        let remote_images = chrome.allow_remote_assets;
        let started = chrome.link.spawn("marknexia-troubleshooting", move || {
            let app = shell_ops::app_folder();
            let sbom_present = shell_ops::sbom_path().is_some_and(|path| {
                GatedPath::new(&path.to_string_lossy()).is_ok()
                    && crate::documents::probe_is_file(&path)
            });
            let local = shell_ops::data_folder();
            Some(WorkerResult::Troubleshooting(DiagnosticsFacts {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                os: shell_ops::os_version(),
                architecture: dotnet_architecture(std::env::consts::ARCH).to_owned(),
                app_path: app
                    .as_deref()
                    .map(shell_ops::with_trailing_separator)
                    .unwrap_or_default(),
                local_data: local
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                webview_data: super::webview_folder().display().to_string(),
                sbom_present,
                remote_images,
            }))
        });
        if !started {
            chrome.diagnostic(TROUBLESHOOTING_FAILED, DiagnosticSeverity::Warning);
        }
    });
}

/// The troubleshooting dialog, with no borrow held (a nested modal loop).
fn show_troubleshooting(hwnd: HWND, facts: &DiagnosticsFacts) {
    if HELP_DIALOG_OPEN.with(|open| open.replace(true)) {
        return;
    }
    let choice = dialogs::show_troubleshooting(hwnd);
    HELP_DIALOG_OPEN.with(|open| open.set(false));
    match choice {
        Some(dialogs::Choice::Primary) => {
            let copied = shell_ops::copy_text(hwnd, &diagnostics_report(facts));
            with_chrome(hwnd, |chrome| match copied {
                Ok(()) => chrome.diagnostic(DIAGNOSTICS_COPIED, DiagnosticSeverity::Info),
                Err(_) => chrome.diagnostic(TROUBLESHOOTING_FAILED, DiagnosticSeverity::Warning),
            });
        }
        Some(dialogs::Choice::Secondary) => with_chrome(hwnd, |chrome| {
            let started = shell_ops::data_folder().is_some_and(|folder| {
                chrome.link.spawn("marknexia-data-folder", move || {
                    Some(WorkerResult::DataFolderOpened(shell_ops::open_folder(
                        &folder,
                    )))
                })
            });
            if !started {
                chrome.diagnostic(TROUBLESHOOTING_FAILED, DiagnosticSeverity::Warning);
            }
        }),
        Some(dialogs::Choice::Close) => {}
        None => with_chrome(hwnd, |chrome| {
            chrome.diagnostic(TROUBLESHOOTING_FAILED, DiagnosticSeverity::Warning);
        }),
    }
}

/// .NET `ShowSbom_Click`: reads `marknexia-sbom.spdx.json` beside the
/// executable on a worker, then shows the dialog.
fn request_sbom(hwnd: HWND) {
    if HELP_DIALOG_OPEN.with(Cell::get) {
        return;
    }
    with_chrome(hwnd, |chrome| {
        let Some(path) = shell_ops::sbom_path() else {
            chrome.diagnostic(SBOM_MISSING, DiagnosticSeverity::Warning);
            return;
        };
        let started = chrome.link.spawn("marknexia-sbom", move || {
            Some(WorkerResult::Sbom(shell_ops::read_sbom(&path)))
        });
        if !started {
            chrome.diagnostic(SBOM_FAILED, DiagnosticSeverity::Warning);
        }
    });
}

/// The software components dialog, with no borrow held.
fn show_sbom(hwnd: HWND, result: Result<(String, Vec<String>), shell_ops::SbomFailure>) {
    let (json, components) = match result {
        Ok(found) => found,
        Err(failure) => {
            let message = match failure {
                shell_ops::SbomFailure::Missing => SBOM_MISSING,
                shell_ops::SbomFailure::Failed => SBOM_FAILED,
            };
            with_chrome(hwnd, |chrome| {
                chrome.diagnostic(message, DiagnosticSeverity::Warning);
            });
            return;
        }
    };
    if HELP_DIALOG_OPEN.with(|open| open.replace(true)) {
        return;
    }
    let choice = dialogs::show_sbom(hwnd, &crate::chrome_model::sbom_rows(&components));
    HELP_DIALOG_OPEN.with(|open| open.set(false));
    let outcome = match choice {
        Some(true) => Some(shell_ops::copy_text(hwnd, &json).is_ok()),
        Some(false) => None,
        None => Some(false),
    };
    if let Some(copied) = outcome {
        with_chrome(hwnd, |chrome| {
            if copied {
                chrome.diagnostic(SBOM_COPIED, DiagnosticSeverity::Info);
            } else {
                chrome.diagnostic(SBOM_FAILED, DiagnosticSeverity::Warning);
            }
        });
    }
}

fn run_command(hwnd: HWND, command: ChromeCommand) {
    match command {
        ChromeCommand::OpenFile => {
            // The existing Open dialog path (multi-select, Markdown filters).
            // SAFETY: posts to this thread's own live window; no pointers.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_OPEN_DIALOG, WPARAM(0), LPARAM(0)) };
        }
        ChromeCommand::OpenFolder => match dialogs::pick_folder(hwnd) {
            Ok(Some(folder)) => open_repository(hwnd, &folder),
            Ok(None) => {}
            Err(error) => with_chrome(hwnd, |chrome| {
                chrome.diagnostic(
                    format!("Open folder dialog failed: {error}"),
                    DiagnosticSeverity::Warning,
                );
            }),
        },
        ChromeCommand::Reload => reload_active(hwnd),
        ChromeCommand::Back => history_step(hwnd, true),
        ChromeCommand::Forward => history_step(hwnd, false),
        ChromeCommand::CloseTab => {
            let _ = execute_command(hwnd, ShellCommand::CloseTab);
        }
        ChromeCommand::ToggleSidebar => toggle_sidebar(hwnd),
        ChromeCommand::ToggleRemoteAssets => toggle_remote_assets(hwnd),
        ChromeCommand::ToggleTextDiagrams => toggle_text_diagrams(hwnd),
        ChromeCommand::SetTheme(theme) => set_theme(hwnd, theme),
        ChromeCommand::ShowHelp => dialogs::show_help(hwnd),
        ChromeCommand::ShowAbout => dialogs::show_about(hwnd),
        ChromeCommand::ShowTroubleshooting => request_troubleshooting(hwnd),
        ChromeCommand::ShowSbom => request_sbom(hwnd),
        ChromeCommand::CheckForUpdates => updater::request_check(hwnd),
    }
}

fn with_chrome(hwnd: HWND, change: impl FnOnce(&mut ChromeState)) {
    if let Some(state) = state_of(hwnd)
        && let Ok(mut state) = state.try_borrow_mut()
    {
        change(&mut state.chrome);
    }
}

fn with_controls(state: &Rc<RefCell<AppState>>, apply: impl FnOnce(&ChromeControls)) {
    // Control handles are copied out so no borrow is held while the
    // controls are sent messages.
    let controls = state
        .try_borrow()
        .ok()
        .and_then(|state| state.chrome.controls);
    if let Some(controls) = controls {
        apply(&controls);
    }
}

fn all_windows(c: &ChromeControls) -> [HWND; 23] {
    [
        c.toolbar.toggle,
        c.toolbar.brand_name,
        c.toolbar.brand_tagline,
        c.toolbar.back,
        c.toolbar.forward,
        c.toolbar.reload,
        c.toolbar.folder,
        c.toolbar.find,
        c.toolbar.theme_label,
        c.toolbar.theme,
        c.sidebar.header,
        c.sidebar.outline_label,
        c.sidebar.tree_label,
        c.sidebar.recent_label,
        c.sidebar.outline,
        c.sidebar.tree,
        c.sidebar.recent,
        c.sidebar.copy_anchor,
        c.sidebar.reveal,
        c.sidebar.splitter,
        c.diagnostics.icon,
        c.diagnostics.text,
        c.diagnostics.close,
    ]
}

/// The chrome fonts: body text for controls and lists, the heading font
/// for the sidebar heading, captions for the status bar and the find
/// count, and the brand fonts for the name and tagline.
fn apply_fonts(
    controls: &ChromeControls,
    shell: ShellControls,
    find: Option<crate::find::native::FindControls>,
    fonts: FontSet,
) {
    for window in all_windows(controls)
        .into_iter()
        .chain([shell.sidebar, shell.find])
    {
        let font = if window == controls.toolbar.brand_name {
            fonts.brand_name
        } else if window == controls.toolbar.brand_tagline {
            fonts.brand_tagline
        } else if window == controls.sidebar.header {
            fonts.heading
        } else {
            fonts.text
        };
        send(window, WM_SETFONT, font.0 as usize, 1);
    }
    send(shell.status, WM_SETFONT, fonts.caption.0 as usize, 1);
    if let Some(find) = find {
        send(find.label, WM_SETFONT, fonts.text.0 as usize, 1);
        send(find.count, WM_SETFONT, fonts.caption.0 as usize, 1);
    }
}

/// Tab order follows the visual order: toolbar, tab strip, sidebar,
/// document, find bar, diagnostics, status.
fn order_controls(c: &ChromeControls, shell: ShellControls) {
    let order = [
        c.toolbar.toggle,
        c.toolbar.brand_name,
        c.toolbar.brand_tagline,
        c.toolbar.back,
        c.toolbar.forward,
        c.toolbar.reload,
        shell.command,
        c.toolbar.folder,
        c.toolbar.find,
        c.toolbar.theme_label,
        c.toolbar.theme,
        shell.tabs,
        shell.sidebar,
        c.sidebar.header,
        // Each list follows its own label: the UIA and MSAA proxies name a
        // list box or tree view from the static label just before it.
        c.sidebar.outline_label,
        c.sidebar.outline,
        c.sidebar.copy_anchor,
        c.sidebar.tree_label,
        c.sidebar.tree,
        c.sidebar.reveal,
        c.sidebar.recent_label,
        c.sidebar.recent,
        c.sidebar.splitter,
        shell.find,
        c.diagnostics.icon,
        c.diagnostics.text,
        c.diagnostics.close,
        shell.status,
    ];
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    // SAFETY: z-order changes among this window's own children.
    let _ = unsafe { SetWindowPos(order[0], Some(HWND_TOP), 0, 0, 0, 0, flags) };
    for pair in order.windows(2) {
        // SAFETY: as above.
        let _ = unsafe { SetWindowPos(pair[1], Some(pair[0]), 0, 0, 0, 0, flags) };
    }
}

/// Tab and Shift+Tab move between the shell's tab stops (the WebView
/// document is reached with F6, as before).
fn move_focus(hwnd: HWND, backward: bool) -> bool {
    // SAFETY: focus queries and changes on this thread's own windows.
    unsafe {
        let focus = GetFocus();
        let current = if focus == hwnd || !IsChild(hwnd, focus).as_bool() {
            None
        } else {
            // A control's inner window (for example a combo box edit) maps
            // to the direct child of the shell window.
            let mut child = focus;
            while let Ok(parent) = windows::Win32::UI::WindowsAndMessaging::GetParent(child) {
                if parent == hwnd {
                    break;
                }
                child = parent;
            }
            Some(child)
        };
        match GetNextDlgTabItem(hwnd, current, backward) {
            Ok(next) => {
                let _ = SetFocus(Some(next));
                true
            }
            Err(_) => false,
        }
    }
}

/// The active tab. Chrome actions run only when neither the shell state
/// nor the portable state is borrowed (`shell_free` in `handle_message`),
/// so the reads cannot conflict; they still never panic.
fn active_tab(state: &Rc<RefCell<AppState>>) -> Option<u64> {
    let portable = crate::reentry::read(state, |state| Rc::clone(&state.portable))?;
    crate::reentry::read(&portable, |app| {
        app.active_tab().map(crate::tabs::TabId::get)
    })
    .flatten()
}

/// .NET `Reload_Click`: re-read and re-render the active document; on
/// failure the previous document stays.
fn reload_active(hwnd: HWND) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let Some(active) = active_tab(&state) else {
        return;
    };
    let path = {
        let mut state = state.borrow_mut();
        let Some(path) = state
            .chrome
            .locations
            .get(&active)
            .map(|location| location.path().to_path_buf())
        else {
            return;
        };
        state.chrome.reloading.insert(active);
        path
    };
    start_render(hwnd, &state, active, path);
    refresh_status(&state);
}

/// Back or forward within the tab's history (.NET `Back_Click`,
/// `Forward_Click`). History entries come from outline jumps in the same
/// document, so the entry is shown by scrolling (CHROME-4).
fn history_step(hwnd: HWND, back: bool) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let Some(active) = active_tab(&state) else {
        return;
    };
    let entry = {
        let mut state = state.borrow_mut();
        state
            .chrome
            .locations
            .get_mut(&active)
            .and_then(|location| {
                if back {
                    location.go_back()
                } else {
                    location.go_forward()
                }
            })
    };
    if let Some(entry) = entry {
        scroll_tab(&state, active, entry.fragment.as_deref());
    }
    sync_ui(hwnd);
}

fn scroll_tab(state: &Rc<RefCell<AppState>>, tab: u64, fragment: Option<&str>) {
    if let Some(session) = take_session(state) {
        if let Some(host) = session.host(tab) {
            let _ = host.scroll_to_anchor(fragment);
        }
        restore_session(state, session);
    }
}

/// .NET `OutlineItem_Click`: the same document at the heading's anchor.
fn outline_jump(hwnd: HWND, index: usize) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let Some(active) = active_tab(&state) else {
        return;
    };
    let target = {
        let mut state = state.borrow_mut();
        let chrome = &mut state.chrome;
        let slug = chrome
            .outlines
            .get(&active)
            .and_then(|(_, entries)| entries.get(index))
            .map(|entry| entry.slug.clone());
        match (slug, chrome.locations.get_mut(&active)) {
            (Some(slug), Some(location)) => location.navigate_to_anchor(&slug).then_some(slug),
            _ => None,
        }
    };
    if let Some(slug) = target {
        scroll_tab(&state, active, Some(&slug));
    }
    sync_ui(hwnd);
}

/// .NET `OpenDocumentInTabAsync(path)` from the recent list, the tree and
/// a repository README: a missing file is reported; an open document is
/// selected and scrolled to its top; otherwise a new tab opens.
pub(super) fn open_or_select(hwnd: HWND, path: PathBuf) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let mut state = state.borrow_mut();
    let chrome = &mut state.chrome;
    if let Err(error) = check_document_path(&path.to_string_lossy()) {
        chrome.diagnostic(
            format!("Unable to open document: {error}"),
            DiagnosticSeverity::Error,
        );
        return;
    }
    // The existence check can block on an offline share.
    let started = chrome.link.spawn("marknexia-open-check", move || {
        // input: identification-only QoS open, after the gate above.
        let exists = crate::documents::probe_is_file(&path);
        Some(WorkerResult::OpenChecked { path, exists })
    });
    if !started {
        chrome.diagnostic(
            "Unable to open document: the file check could not start.",
            DiagnosticSeverity::Error,
        );
    }
}

/// The rest of `open_or_select` once the file is known to exist: an open
/// document is selected and scrolled to its top; otherwise a new tab opens.
fn finish_open(hwnd: HWND, path: PathBuf, exists: bool) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    if !exists {
        state.borrow_mut().chrome.diagnostic(
            format!("File not found: {}", path.display()),
            DiagnosticSeverity::Error,
        );
        return;
    }
    let existing = {
        let state = state.borrow();
        state
            .chrome
            .locations
            .iter()
            .find(|(_, location)| same_document_path(location.path(), &path))
            .map(|(tab, _)| *tab)
    };
    let open_tabs: Vec<u64> = {
        let portable = Rc::clone(&state.borrow().portable);
        portable
            .borrow()
            .tabs()
            .tabs()
            .iter()
            .map(|tab| tab.id().get())
            .collect()
    };
    match existing.filter(|tab| open_tabs.contains(tab)) {
        Some(tab) => {
            {
                let mut state = state.borrow_mut();
                let chrome = &mut state.chrome;
                // .NET adds the path again; only a document that rendered.
                if chrome.outlines.contains_key(&tab) {
                    chrome.add_recent(&path);
                }
            }
            if !select_tab_id(hwnd, tab, false) {
                // The worker result can arrive in a nested pump while the
                // WebView session is checked out (a WebView2 call in
                // progress, slower on ARM64); the switch then fails and was
                // dropped. Apply it from the message loop once the session
                // is back, as for a deferred UIA selection.
                let id = state
                    .borrow()
                    .portable
                    .borrow()
                    .tabs()
                    .tabs()
                    .iter()
                    .find_map(|candidate| (candidate.id().get() == tab).then(|| candidate.id()));
                if let Some(id) = id {
                    super::input::defer_automation_select(id);
                }
            }
            let changed = state
                .borrow_mut()
                .chrome
                .locations
                .get_mut(&tab)
                .is_some_and(|location| location.navigate_to(None));
            if changed {
                scroll_tab(&state, tab, None);
            }
            sync_ui(hwnd);
        }
        None => open_document(hwnd, path),
    }
}

/// .NET `OpenRepositoryFolderAsync`: the folder passes the root gate here;
/// its existence and README are checked on a worker.
pub(super) fn open_repository(hwnd: HWND, folder: &Path) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let mut state = state.borrow_mut();
    let chrome = &mut state.chrome;
    if let Err(error) = GatedPath::new(&folder.to_string_lossy()) {
        chrome.diagnostic(
            format!("{}", ScanError::InvalidRoot(error)),
            DiagnosticSeverity::Error,
        );
        return;
    }
    let folder = folder.to_path_buf();
    let started = chrome.link.spawn("marknexia-folder-check", move || {
        // input: identification-only QoS opens, after the gates.
        let exists = crate::documents::probe_is_dir(&folder);
        let readme = folder.join("README.md");
        let readme = (exists
            && check_document_path(&readme.to_string_lossy()).is_ok()
            && crate::documents::probe_is_file(&readme))
        .then_some(readme);
        Some(WorkerResult::FolderChecked {
            folder,
            exists,
            readme,
        })
    });
    if !started {
        chrome.diagnostic(REPOSITORY_LOAD_FAILED, DiagnosticSeverity::Warning);
    }
}

fn finish_open_repository(hwnd: HWND, folder: &Path, exists: bool, readme: Option<PathBuf>) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    if !exists {
        state.borrow_mut().chrome.diagnostic(
            format!("Folder not found: {}", folder.display()),
            DiagnosticSeverity::Error,
        );
        return;
    }
    let (selector, portable) = {
        let mut state = state.borrow_mut();
        let selector = state.controls.map(|c| c.sidebar);
        let portable = Rc::clone(&state.portable);
        let chrome = &mut state.chrome;
        let root = folder.to_string_lossy().into_owned();
        // A detection still running for an earlier document must not
        // replace the chosen folder.
        chrome.cancel_root_detection();
        chrome.repository_root = Some(folder.to_path_buf());
        chrome.save(|settings| {
            settings.repository_root = Some(root.clone());
            settings.sidebar_mode = 1;
        });
        if let Some(error) = settings_bridge::add_recent_folder(chrome.store.as_mut(), &root) {
            chrome.diagnostics.show(error, DiagnosticSeverity::Warning);
        }
        chrome.start_tree(folder);
        chrome.mode = SidebarMode::Repository;
        (selector, portable)
    };
    // .NET opens the pane without saving `IsSidebarOpen`.
    portable.borrow_mut().set_sidebar_visible(true);
    if let Some(selector) = selector {
        sidebar::select_mode(selector, SidebarMode::Repository);
    }
    relayout(hwnd, &state);
    if let Some(readme) = readme {
        finish_open(hwnd, readme, true);
    }
    sync_ui(hwnd);
}

fn relayout(hwnd: HWND, state: &Rc<RefCell<AppState>>) {
    let dpi = state.borrow().dpi;
    reflow(hwnd, dpi);
}

fn set_mode(hwnd: HWND, mode: SidebarMode) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    {
        let mut state = state.borrow_mut();
        let chrome = &mut state.chrome;
        if chrome.mode == mode {
            return;
        }
        chrome.mode = mode;
        if let Some(value) = mode.setting_value() {
            chrome.save(|settings| settings.sidebar_mode = value);
        }
        if mode == SidebarMode::Recent {
            chrome.recent_dirty = true;
        }
    }
    relayout(hwnd, &state);
    sync_ui(hwnd);
}

/// .NET `ToggleSidebar_Click`: toggles and saves `IsSidebarOpen`.
fn toggle_sidebar(hwnd: HWND) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let portable = Rc::clone(&state.borrow().portable);
    let visible = !portable.borrow().sidebar_visible();
    portable.borrow_mut().set_sidebar_visible(visible);
    state
        .borrow_mut()
        .chrome
        .save(|settings| settings.is_sidebar_open = visible);
    relayout(hwnd, &state);
}

/// .NET `RemoteAssetsMenuItem_Click`.
fn toggle_remote_assets(hwnd: HWND) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    {
        let mut state = state.borrow_mut();
        let chrome = &mut state.chrome;
        let enabled = !chrome.allow_remote_assets;
        chrome.allow_remote_assets = enabled;
        chrome.save(|settings| settings.allow_remote_assets = enabled);
        chrome.diagnostic(remote_assets_notice(enabled), DiagnosticSeverity::Info);
    }
    reload_active(hwnd);
    sync_ui(hwnd);
}

/// View > Render text diagrams (Rust edition only, TDG-8): saves the
/// setting and reloads the active document, like the remote-images toggle.
fn toggle_text_diagrams(hwnd: HWND) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    {
        let mut state = state.borrow_mut();
        let chrome = &mut state.chrome;
        let enabled = !chrome.render_text_diagrams;
        chrome.render_text_diagrams = enabled;
        chrome.save(|settings| settings.render_text_diagrams = enabled);
        chrome.diagnostic(text_diagrams_notice(enabled), DiagnosticSeverity::Info);
    }
    reload_active(hwnd);
    sync_ui(hwnd);
}

/// .NET `ThemeSelector_SelectionChanged`.
fn set_theme(hwnd: HWND, theme: ThemePreference) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let portable = Rc::clone(&state.borrow().portable);
    if portable.borrow().theme() == theme {
        return;
    }
    portable.borrow_mut().set_theme(theme);
    state
        .borrow_mut()
        .chrome
        .save(|settings| settings.theme = to_stored(theme));
    update_theme(hwnd);
    relayout(hwnd, &state);
    reload_active(hwnd);
    // The start page follows the theme too.
    super::rerender_shell_pages(hwnd);
}

/// Splitter drag start (.NET `SidebarResizeHandle_PointerPressed`).
pub(super) fn begin_sidebar_drag(hwnd: HWND) {
    // A press while an outer frame holds the state starts no drag (the
    // splitter keeps its capture until the release, which ends nothing).
    if state_of(hwnd).is_some_and(|state| !super::shell_free(&state)) {
        return;
    }
    let visible = state_of(hwnd).is_some_and(|state| {
        let portable = Rc::clone(&state.borrow().portable);
        portable.borrow().sidebar_visible()
    });
    // The compact pane keeps the .NET length (its handle is hidden).
    with_chrome(hwnd, |chrome| {
        chrome.dragging = visible && chrome.compact != Some(true);
    });
    drag_sidebar(hwnd);
}

/// .NET `UpdateSidebarWidth`: the pane width follows the pointer.
pub(super) fn drag_sidebar(hwnd: HWND) {
    // Moves while an outer frame holds the state are ignored; the next move
    // continues the drag from the pointer position.
    let Some(state) = state_of(hwnd).filter(|state| super::shell_free(state)) else {
        return;
    };
    let (dragging, dpi) = {
        let state = state.borrow();
        (state.chrome.dragging, state.dpi)
    };
    if !dragging {
        return;
    }
    let mut point = windows::Win32::Foundation::POINT::default();
    // SAFETY: point conversion for this thread's own window.
    let converted = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point).is_ok()
            && windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point).as_bool()
    };
    if !converted {
        return;
    }
    let width = clamp_sidebar_width(f64::from(point.x) * 96.0 / f64::from(dpi.max(1)));
    let changed = {
        let mut state = state.borrow_mut();
        let changed = sidebar_width_dip(state.chrome.sidebar_width) != sidebar_width_dip(width);
        state.chrome.sidebar_width = width;
        changed
    };
    if changed {
        reflow(hwnd, dpi);
    }
}

/// .NET `StopSidebarResize`: saves the width.
pub(super) fn end_sidebar_drag(hwnd: HWND) {
    if state_of(hwnd).is_some_and(|state| !super::shell_free(&state)) {
        // Never left dragging: the end runs from the message loop.
        super::input::defer_sidebar_drag_end();
        return;
    }
    with_chrome(hwnd, |chrome| {
        if chrome.dragging {
            chrome.dragging = false;
            let width = chrome.sidebar_width;
            chrome.save(|settings| settings.sidebar_width = width);
        }
    });
}

/// Keyboard resize on the focused splitter; saved after each step.
pub(super) fn resize_sidebar_by_key(hwnd: HWND, step: SidebarResize) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    if !super::shell_free(&state) {
        super::input::defer_sidebar_key(step);
        return;
    }
    let (portable, compact) = {
        let state = state.borrow();
        (
            Rc::clone(&state.portable),
            state.chrome.compact == Some(true),
        )
    };
    if compact || !portable.borrow().sidebar_visible() {
        return;
    }
    let dpi = {
        let mut state = state.borrow_mut();
        let width = resize_sidebar(state.chrome.sidebar_width, step);
        state.chrome.sidebar_width = width;
        state.chrome.save(|settings| settings.sidebar_width = width);
        state.dpi
    };
    reflow(hwnd, dpi);
}

pub(super) fn apply_worker_results(hwnd: HWND) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let results: Vec<WorkerResult> = state.borrow().chrome.worker_results.try_iter().collect();
    for result in results {
        // Opening continues outside the borrow: it selects tabs, starts
        // renders and talks to WebView2.
        match result {
            WorkerResult::OpenChecked { path, exists } => {
                finish_open(hwnd, path, exists);
                continue;
            }
            WorkerResult::FolderChecked {
                folder,
                exists,
                readme,
            } => {
                finish_open_repository(hwnd, &folder, exists, readme);
                continue;
            }
            // The dialogs run nested modal loops: they run as chrome actions
            // after this batch, so the modal pump never re-enters it.
            WorkerResult::Troubleshooting(facts) => {
                queue_action(ChromeAction::ShowTroubleshooting(facts));
                continue;
            }
            WorkerResult::Sbom(result) => {
                queue_action(ChromeAction::ShowSbom(result));
                continue;
            }
            WorkerResult::Update(result) => {
                updater::deliver(result);
                continue;
            }
            result => apply_worker_result(&mut state.borrow_mut().chrome, result),
        }
    }
    sync_ui(hwnd);
}

fn apply_worker_result(chrome: &mut ChromeState, result: WorkerResult) {
    match result {
        WorkerResult::Tree { generation, result } if generation == chrome.tree_generation => {
            chrome.tree_cancel = None;
            match result {
                Ok(arena) => {
                    if arena.incomplete() {
                        chrome
                            .diagnostics
                            .show(REPOSITORY_INCOMPLETE_NOTICE, DiagnosticSeverity::Warning);
                    }
                    chrome.tree = arena;
                    chrome.tree_populated.clear();
                    chrome.tree_populated.insert(0);
                    chrome.tree_dirty = true;
                }
                Err(ScanError::Cancelled) => {}
                Err(_) => chrome
                    .diagnostics
                    .show(REPOSITORY_LOAD_FAILED, DiagnosticSeverity::Warning),
            }
        }
        WorkerResult::DetectedRoot(root) => {
            chrome.detecting_root = false;
            chrome.root_cancel = None;
            // .NET: the first document with a repository sets the root
            // when none is open, and saves it.
            if chrome.repository_root.is_none()
                && let Some(root) = root
            {
                let text = root.to_string_lossy().into_owned();
                chrome.repository_root = Some(root.clone());
                chrome.save(|settings| settings.repository_root = Some(text));
                chrome.start_tree(&root);
            }
        }
        WorkerResult::RootChecked { root, exists } => {
            if !exists {
                chrome.save(|settings| settings.repository_root = None);
            } else if chrome.repository_root.is_none() {
                chrome.repository_root = Some(root.clone());
                chrome.start_tree(&root);
            }
        }
        WorkerResult::Recent {
            generation,
            visible,
        } if generation == chrome.recent_generation => {
            chrome.shown_recent = visible;
            chrome.recent_fill = true;
        }
        WorkerResult::Revealed(Err(error)) => {
            chrome.diagnostic(reveal_failed_notice(&error), DiagnosticSeverity::Warning)
        }
        WorkerResult::DataFolderOpened(Err(_)) => {
            chrome.diagnostic(TROUBLESHOOTING_FAILED, DiagnosticSeverity::Warning);
        }
        WorkerResult::Tree { .. }
        | WorkerResult::Recent { .. }
        | WorkerResult::OpenChecked { .. }
        | WorkerResult::FolderChecked { .. }
        | WorkerResult::Troubleshooting(_)
        | WorkerResult::Sbom(_)
        | WorkerResult::Update(_)
        | WorkerResult::Revealed(Ok(()))
        | WorkerResult::DataFolderOpened(Ok(())) => {}
    }
}

/// The tree root and its children, copied for filling the control.
type TreeSnapshot = (TreeNode, Vec<(usize, TreeNode)>);

/// Gives the chrome controls their accessible names. List boxes, tree
/// views, combo boxes and tab controls take their MSAA/UIA name from a
/// preceding label, not their window text, so the names are set as
/// dynamic annotations (`IAccPropServices`). The diagnostics message is an
/// owner-drawn static, which oleacc would report as a graphic: its role is
/// set to static text. Runs after the STA is entered; without COM the
/// controls keep the platform defaults.
fn annotate_controls(state: &Rc<RefCell<AppState>>) {
    let (controls, shell) = {
        let state = state.borrow();
        (state.chrome.controls, state.controls)
    };
    let Some(controls) = controls else {
        return;
    };
    // SAFETY: in-process COM object created on the UI thread's STA.
    let Ok(names) = (unsafe {
        CoCreateInstance::<_, IAccPropServices>(&CAccPropServices, None, CLSCTX_INPROC_SERVER)
    }) else {
        return;
    };
    let mut named = vec![
        (controls.sidebar.outline, SidebarMode::Outline.list_name()),
        (controls.sidebar.tree, SidebarMode::Repository.list_name()),
        (controls.sidebar.recent, SidebarMode::Recent.list_name()),
        (controls.sidebar.splitter, "Resize sidebar"),
        (controls.toolbar.theme, "Theme"),
    ];
    if let Some(shell) = shell {
        named.push((shell.sidebar, "Sidebar view"));
        // An edit control is named by a preceding label, which the find bar
        // has none of (.NET `SearchInputBox` name).
        named.push((shell.find, "Find in document"));
    }
    for (window, name) in named {
        set_accessible_name(&names, window, name);
    }
    for label in [
        controls.sidebar.outline_label,
        controls.sidebar.tree_label,
        controls.sidebar.recent_label,
    ] {
        // The list carries the name; its label is not a separate element.
        for property in [
            IsControlElement_Property_GUID,
            IsContentElement_Property_GUID,
        ] {
            // SAFETY: as above; the VARIANT outlives the call.
            let _ = unsafe {
                names.SetHwndProp(
                    label,
                    OBJID_CLIENT.0 as u32,
                    CHILDID_SELF,
                    property,
                    &VARIANT::from(false),
                )
            };
        }
    }
    state.borrow_mut().chrome.names = Some(names);
}

fn set_accessible_name(names: &IAccPropServices, window: HWND, name: &str) {
    let text = wide(name);
    // The MSAA name and the UI Automation Name (direct annotation of UIA
    // properties, Windows 8 and later).
    for property in [PROPID_ACC_NAME, Name_Property_GUID] {
        // SAFETY: the buffer is NUL-terminated and outlives the call
        // (oleacc copies it); no callback into the shell.
        let _ = unsafe {
            names.SetHwndPropStr(
                window,
                OBJID_CLIENT.0 as u32,
                CHILDID_SELF,
                property,
                PCWSTR::from_raw(text.as_ptr()),
            )
        };
    }
}

/// Brings every chrome control in line with the state. Lists are refilled
/// only when their content changed.
fn sync_ui(hwnd: HWND) {
    let Some(state) = state_of(hwnd) else {
        return;
    };
    let Some(active) = state.try_borrow().ok().map(|_| active_tab(&state)) else {
        return;
    };
    let portable = Rc::clone(&state.borrow().portable);
    let theme = portable.borrow().theme();
    struct Snapshot {
        controls: Option<ChromeControls>,
        selector: Option<HWND>,
        outline: Option<Vec<OutlineEntry>>,
        recent: Option<Vec<String>>,
        tree: Option<Option<TreeSnapshot>>,
        back: bool,
        forward: bool,
        reload: bool,
        remote: bool,
        text_diagrams: bool,
        mode: SidebarMode,
        header: &'static str,
        diagnostic: Option<String>,
        announce: Option<String>,
        severity: Option<&'static str>,
        tree_generation: u32,
        relayout: bool,
        copy_anchor: bool,
        reveal: bool,
    }
    let open_tabs: BTreeSet<u64> = portable
        .borrow()
        .tabs()
        .tabs()
        .iter()
        .map(|tab| tab.id().get())
        .collect();
    let snapshot = {
        let Ok(mut state_ref) = state.try_borrow_mut() else {
            return;
        };
        let selector = state_ref.controls.map(|c| c.sidebar);
        let chrome = &mut state_ref.chrome;
        // Forget closed tabs (tab ids are never reused).
        chrome.locations.retain(|tab, _| open_tabs.contains(tab));
        chrome.outlines.retain(|tab, _| open_tabs.contains(tab));
        chrome.reloading.retain(|tab| open_tabs.contains(tab));
        chrome.pending_scroll.retain(|tab| open_tabs.contains(tab));
        let outline_key = active.map(|tab| {
            (
                tab,
                chrome
                    .outlines
                    .get(&tab)
                    .map_or(0, |(generation, _)| *generation),
            )
        });
        let outline = (outline_key != chrome.shown_outline).then(|| {
            chrome.shown_outline = outline_key;
            active
                .and_then(|tab| chrome.outlines.get(&tab))
                .map(|(_, entries)| entries.clone())
                .unwrap_or_default()
        });
        if chrome.recent_dirty {
            chrome.start_recent_listing();
        }
        let recent = std::mem::take(&mut chrome.recent_fill).then(|| chrome.shown_recent.clone());
        let tree = chrome.tree_dirty.then(|| {
            chrome.tree_dirty = false;
            // Only the low 32 bits travel in item lParams.
            chrome.tree_shown = chrome.tree_generation as u32;
            chrome.tree.root().map(|root| {
                let children = root
                    .children
                    .iter()
                    .filter_map(|index| chrome.tree.node(*index).map(|n| (*index, n.clone())))
                    .collect();
                (root.clone(), children)
            })
        });
        let header = match chrome.mode {
            SidebarMode::Repository => repository_header(chrome.tree.incomplete()),
            mode => mode.header(),
        };
        let open = chrome.diagnostics.is_open();
        let diagnostic = chrome
            .diagnostics
            .current()
            .map(crate::chrome_model::DiagnosticMessage::accessible_name);
        // Announce each new message once.
        let announce = diagnostic
            .as_ref()
            .filter(|text| chrome.announced.as_ref() != Some(*text))
            .cloned();
        chrome.announced.clone_from(&diagnostic);
        let location = chrome.active_location(active);
        Snapshot {
            controls: chrome.controls,
            selector,
            outline,
            recent,
            tree,
            back: location.is_some_and(TabLocation::can_go_back),
            forward: location.is_some_and(TabLocation::can_go_forward),
            reload: location.is_some(),
            remote: chrome.allow_remote_assets,
            text_diagrams: chrome.render_text_diagrams,
            mode: chrome.mode,
            header,
            diagnostic,
            announce,
            severity: chrome
                .diagnostics
                .current()
                .map(crate::chrome_model::DiagnosticMessage::severity_label),
            tree_generation: chrome.tree_shown,
            relayout: open != chrome.diagnostics_laid_out,
            // The row buttons act on the selected row of a non-empty list.
            copy_anchor: active
                .and_then(|tab| chrome.outlines.get(&tab))
                .is_some_and(|(_, entries)| !entries.is_empty()),
            reveal: chrome.tree.root().is_some(),
        }
    };
    let Some(controls) = snapshot.controls else {
        return;
    };
    if let Some(entries) = snapshot.outline {
        sidebar::fill_outline(controls.sidebar.outline, &entries);
    }
    if let Some(paths) = snapshot.recent {
        sidebar::fill_recent(controls.sidebar.recent, &paths);
    }
    if let Some(tree) = snapshot.tree {
        sidebar::fill_tree(
            controls.sidebar.tree,
            snapshot.tree_generation,
            tree.as_ref().map(|(root, children)| {
                (
                    root,
                    children
                        .iter()
                        .map(|(index, node)| (*index, node))
                        .collect(),
                )
            }),
        );
    }
    // SAFETY: enable state, menu check and combo selection of this thread's
    // own controls; no borrow is held.
    unsafe {
        let _ = EnableWindow(controls.toolbar.back, snapshot.back);
        let _ = EnableWindow(controls.toolbar.forward, snapshot.forward);
        let _ = EnableWindow(controls.toolbar.reload, snapshot.reload);
        let _ = EnableWindow(controls.sidebar.copy_anchor, snapshot.copy_anchor);
        let _ = EnableWindow(controls.sidebar.reveal, snapshot.reveal);
        if let Some(menu) = controls.menu {
            let check = if snapshot.remote {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
            CheckMenuItem(menu, MENU_REMOTE_ASSETS as u32, (MF_BYCOMMAND | check).0);
            let check = if snapshot.text_diagrams {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
            CheckMenuItem(menu, MENU_TEXT_DIAGRAMS as u32, (MF_BYCOMMAND | check).0);
        }
    }
    send(controls.toolbar.theme, CB_SETCURSEL, theme_index(theme), 0);
    if let Some(selector) = snapshot.selector {
        sidebar::select_mode(selector, snapshot.mode);
    }
    set_text(controls.sidebar.header, snapshot.header);
    set_text(
        controls.diagnostics.text,
        snapshot.diagnostic.as_deref().unwrap_or(""),
    );
    set_text(controls.diagnostics.icon, snapshot.severity.unwrap_or(""));
    // SAFETY: repaint requests for children of this thread.
    unsafe {
        let _ = InvalidateRect(Some(controls.diagnostics.icon), None, true);
        let _ = InvalidateRect(Some(controls.diagnostics.text), None, true);
    }
    if snapshot.relayout {
        relayout(hwnd, &state);
    }
    if let Some(text) = snapshot.announce {
        diagnostics::announce(controls.diagnostics.text, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Control ids are AutomationIds and `WM_COMMAND` sources: every
    /// shell child needs its own (the brand ids once collided with the
    /// runtime state's 2030 to 2033).
    #[test]
    fn shell_control_ids_are_unique() {
        let mut ids = vec![
            super::super::COMMAND_OPEN_ID,
            ID_SELECTOR,
            ID_TOGGLE_SIDEBAR,
            ID_BACK,
            ID_FORWARD,
            ID_RELOAD,
            ID_OPEN_FOLDER,
            ID_THEME_LABEL,
            ID_THEME,
            ID_FIND,
            ID_SIDEBAR_HEADER,
            ID_OUTLINE,
            ID_TREE,
            ID_RECENT,
            ID_SPLITTER,
            ID_OUTLINE_LABEL,
            ID_TREE_LABEL,
            ID_RECENT_LABEL,
            ID_COPY_ANCHOR,
            ID_REVEAL,
            ID_DIAGNOSTIC_TEXT,
            ID_DIAGNOSTIC_CLOSE,
            ID_DIAGNOSTIC_ICON,
            ID_BRAND_NAME,
            ID_BRAND_TAGLINE,
            super::super::runtime::ID_RUNTIME_TEXT,
            super::super::runtime::ID_RUNTIME_INSTALL,
            super::super::runtime::ID_RUNTIME_CANCEL,
            super::super::runtime::ID_RUNTIME_LINK,
        ];
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate shell control ids");
    }
}
