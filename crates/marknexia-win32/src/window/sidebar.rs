//! The sidebar (.NET `MainSplitView.Pane`): a view selector, a heading, the
//! document outline, the repository tree and the recent files, and the
//! resize handle. Every control is a native comctl32/user32 control, so UI
//! Automation comes from the platform proxies.
//!
//! Every function that sends a message to a control is called without an
//! `AppState` borrow: controls notify the parent window synchronously.

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, DT_BOTTOM, DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
            DT_SINGLELINE, DT_TOP, DT_VCENTER, DrawFocusRect, EndPaint, HFONT, HGDIOBJ,
            InvalidateRect, PAINTSTRUCT, ScreenToClient, SelectObject, SetBkMode, SetTextColor,
            TRANSPARENT,
        },
        UI::{
            Controls::{
                DRAWITEMSTRUCT, HTREEITEM, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED, TCITEMW,
                TVI_LAST, TVI_ROOT, TVIF_CHILDREN, TVIF_PARAM, TVIF_TEXT, TVINSERTSTRUCTW,
                TVINSERTSTRUCTW_0, TVITEMEXW_CHILDREN, TVITEMW,
            },
            Input::KeyboardAndMouse::{GetFocus, ReleaseCapture, SetCapture},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                DefWindowProcW, GetClientRect, GetCursorPos, GetParent, IDC_SIZEWE, LoadCursorW,
                RegisterClassW, SetCursor, WINDOW_STYLE, WM_CAPTURECHANGED, WM_ERASEBKGND,
                WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY,
                WM_PAINT, WM_SETCURSOR, WM_SETFOCUS, WNDCLASSW, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
            },
        },
    },
    core::{PCWSTR, PWSTR, w},
};

use super::{
    WindowError, app_state_handle, child_style,
    chrome::{
        self, ID_COPY_ANCHOR, ID_OUTLINE, ID_OUTLINE_LABEL, ID_RECENT, ID_RECENT_LABEL, ID_REVEAL,
        ID_SIDEBAR_HEADER, ID_SPLITTER, ID_TREE, ID_TREE_LABEL, SidebarControls, dip, send,
        set_bounds, wide,
    },
    create_child, fill, native_color,
    style::{self, SidebarColors},
};
use crate::{
    chrome_model::{OutlineEntry, SidebarMode, TreeNode, sidebar_resize_key},
    layout::PixelRect,
    theme::ColorSpec,
};

pub(super) const SPLITTER_CLASS: PCWSTR = w!("MarknexiaRustSplitter");

const TCS_FIXEDWIDTH: u32 = 0x0400;
const TCS_FOCUSONBUTTONDOWN: u32 = 0x1000;
const TCIF_TEXT: u32 = 0x1;
const TCM_INSERTITEMW: u32 = 0x133E;
const TCM_SETCURSEL: u32 = 0x130C;
const TCM_SETITEMSIZE: u32 = 0x1329;
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;
const LBS_NOTIFY: u32 = 0x1;
const LBS_OWNERDRAWFIXED: u32 = 0x10;
const LBS_HASSTRINGS: u32 = 0x40;
const LBS_NOINTEGRALHEIGHT: u32 = 0x100;
const WS_VSCROLL: u32 = 0x0020_0000;
const TVS_HASBUTTONS: u32 = 0x1;
const TVS_LINESATROOT: u32 = 0x4;
const TVS_SHOWSELALWAYS: u32 = 0x20;
const LB_ADDSTRING: u32 = 0x0180;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_GETCURSEL: u32 = 0x0188;
const LB_GETTEXT: u32 = 0x0189;
const LB_GETTEXTLEN: u32 = 0x018A;
const LB_SETITEMDATA: u32 = 0x019A;
const LB_SETITEMHEIGHT: u32 = 0x01A0;
const TVM_INSERTITEMW: u32 = 0x1132;
const TVM_DELETEITEM: u32 = 0x1101;
const TVM_EXPAND: u32 = 0x1102;
const TVM_SETITEMHEIGHT: u32 = 0x111B;
const TVM_GETNEXTITEM: u32 = 0x110A;
const TVM_GETITEMW: u32 = 0x113E;
const TVM_HITTEST: u32 = 0x1111;
const TVE_EXPAND: usize = 0x2;
const TVGN_CARET: usize = 0x9;
/// `TVHT_ONITEMICON | TVHT_ONITEMLABEL | TVHT_ONITEMSTATEICON`.
const TVHT_ONITEM: u32 = 0x46;
/// Height of the heading row above the list, in DIPs: it holds the
/// 44-DIP row action button.
const HEADER_HEIGHT: u32 = 44;
/// The left inset of the heading and of the list text, in DIPs, so both
/// start on the same line (.NET `DocumentMapHeader` margin 16).
const TEXT_INSET: u32 = 16;
/// Repository tree rows (WinUI `TreeViewItem` is 32 DIPs; CHROME-18).
const TREE_ROW: u32 = 32;
const TVS_FULLROWSELECT: u32 = 0x1000;
const TVS_TRACKSELECT: u32 = 0x200;
const TVM_SETINDENT: u32 = 0x1107;
const TVM_SETEXTENDEDSTYLE: u32 = 0x112C;
const TVS_EX_DOUBLEBUFFER: isize = 0x4;
/// Hides the expand chevrons while the pointer is outside the tree. Not
/// used: the WinUI `TreeView` of .NET always shows them, and with it the
/// chevrons vanished as soon as the pointer moved to the document (for
/// example to follow a link), which looked like a lost tree.
#[cfg(test)]
const TVS_EX_FADEINOUTEXPANDOS: isize = 0x40;
const TREE_EXTENDED: isize = TVS_EX_DOUBLEBUFFER;
const BS_OWNERDRAW: u32 = 0x0B;

/// The view selector (.NET `SidebarModeSelector`, a two-tab `TabView`; the
/// third tab is CHROME-1). It is the `ShellControls::sidebar` control, so F6
/// focus lands on it. Named "Sidebar view" as in .NET.
pub(super) fn create_mode_selector(parent: HWND, instance: HINSTANCE) -> Result<HWND, WindowError> {
    use windows::Win32::UI::Controls::{
        ICC_STANDARD_CLASSES, ICC_TAB_CLASSES, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX,
        InitCommonControlsEx,
    };
    let classes = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_STANDARD_CLASSES | ICC_TAB_CLASSES | ICC_TREEVIEW_CLASSES,
    };
    // SAFETY: registers the comctl32 classes used by the sidebar before any
    // of them is created; the struct is a live local.
    let _ = unsafe { InitCommonControlsEx(&classes) };
    let selector = create_child(
        parent,
        instance,
        w!("SysTabControl32"),
        w!("Sidebar view"),
        WINDOW_STYLE(
            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | TCS_FIXEDWIDTH | TCS_FOCUSONBUTTONDOWN,
        ),
        1003,
    )?;
    for (index, mode) in SidebarMode::ALL.iter().enumerate() {
        let mut text = wide(mode.label());
        let item = TCITEMW {
            mask: windows::Win32::UI::Controls::TCITEMHEADERA_MASK(TCIF_TEXT),
            pszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        send(
            selector,
            TCM_INSERTITEMW,
            index,
            std::ptr::from_ref(&item) as isize,
        );
    }
    Ok(selector)
}

pub(super) fn create(parent: HWND, instance: HINSTANCE) -> Result<SidebarControls, WindowError> {
    let header = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!("Document Map"),
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | SS_NOPREFIX | SS_CENTERIMAGE),
        ID_SIDEBAR_HEADER as isize,
    )?;
    // The UIA and MSAA proxies name a list box or tree view from the
    // static label just before it in z-order (`chrome::order_controls`).
    // These zero-size labels carry the .NET `AutomationProperties.Name`;
    // the visible heading keeps the .NET heading text ("Document Map").
    let label = |text: PCWSTR, id: usize| {
        create_child(
            parent,
            instance,
            w!("STATIC"),
            text,
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | SS_NOPREFIX),
            id as isize,
        )
    };
    let outline_label = label(w!("Document outline"), ID_OUTLINE_LABEL)?;
    let tree_label = label(w!("Repository files"), ID_TREE_LABEL)?;
    let recent_label = label(w!("Recent Markdown files"), ID_RECENT_LABEL)?;
    let list_style = WS_CHILD.0 | WS_TABSTOP.0 | WS_VSCROLL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT;
    let outline = create_child(
        parent,
        instance,
        w!("LISTBOX"),
        w!("Document outline"),
        WINDOW_STYLE(list_style | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS),
        ID_OUTLINE as isize,
    )?;
    let tree = create_child(
        parent,
        instance,
        w!("SysTreeView32"),
        w!("Repository files"),
        WINDOW_STYLE(
            // Explorer-style rows: chevrons, no lines, full-row hover and
            // selection (the theme draws them).
            WS_CHILD.0
                | WS_TABSTOP.0
                | TVS_HASBUTTONS
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS
                | TVS_FULLROWSELECT
                | TVS_TRACKSELECT,
        ),
        ID_TREE as isize,
    )?;
    let recent = create_child(
        parent,
        instance,
        w!("LISTBOX"),
        w!("Recent Markdown files"),
        // Owner-drawn (name above folder); the item text, which names the
        // UI Automation item, stays the full path.
        WINDOW_STYLE(list_style | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS),
        ID_RECENT as isize,
    )?;
    send(
        tree,
        TVM_SETEXTENDEDSTYLE,
        TREE_EXTENDED as usize,
        TREE_EXTENDED,
    );
    // The .NET row buttons (CHROME-3), one per view: each acts on the
    // selected row of its list. Shown with their view by `layout`.
    let action = |name: PCWSTR, id: usize| {
        create_child(
            parent,
            instance,
            w!("BUTTON"),
            name,
            WINDOW_STYLE(child_style(BS_OWNERDRAW).0 & !WS_VISIBLE.0),
            id as isize,
        )
    };
    let copy_anchor = action(w!("Copy heading anchor"), ID_COPY_ANCHOR)?;
    let reveal = action(w!("Reveal in File Explorer"), ID_REVEAL)?;
    let splitter = create_child(
        parent,
        instance,
        SPLITTER_CLASS,
        w!("Resize sidebar"),
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
        ID_SPLITTER as isize,
    )?;
    Ok(SidebarControls {
        header,
        outline_label,
        tree_label,
        recent_label,
        outline,
        tree,
        recent,
        copy_anchor,
        reveal,
        splitter,
    })
}

pub(super) fn register_splitter(instance: HINSTANCE) -> Result<(), WindowError> {
    // SAFETY: the class name is static and the cursor is a shared system
    // cursor.
    let cursor = unsafe { LoadCursorW(None, IDC_SIZEWE) }.map_err(WindowError::Cursor)?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(splitter_proc),
        hInstance: instance,
        lpszClassName: SPLITTER_CLASS,
        hCursor: cursor,
        ..Default::default()
    };
    // SAFETY: registers a class whose procedure is defined below.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(WindowError::Register(windows::core::Error::from_thread()));
    }
    Ok(())
}

/// Lays the sidebar out: selector (44 DIPs), heading, the current view's
/// list; the splitter on the right edge. Only the current view is visible.
pub(super) fn layout(
    selector: HWND,
    controls: &SidebarControls,
    sidebar: PixelRect,
    splitter: PixelRect,
    mode: SidebarMode,
    dpi: u32,
) {
    let width = sidebar.width.saturating_sub(splitter.width);
    let target = dip(44, dpi);
    // The tab items keep the 44-DIP target; the divider sits below them.
    let selector_height = sidebar.height.min(target.saturating_add(dip(4, dpi)));
    set_bounds(
        selector,
        PixelRect {
            width,
            height: selector_height,
            ..sidebar
        },
    );
    // Equal pivot items across the pane (the painted labels are centred),
    // always inside the control, so its scroll arrows never appear.
    let item_width = selector_item_width(width, dpi);
    send(
        selector,
        TCM_SETITEMSIZE,
        0,
        make_lparam(item_width, target),
    );
    let header_height = sidebar
        .height
        .saturating_sub(selector_height)
        .min(dip(HEADER_HEIGHT, dpi));
    let header_inset = dip(TEXT_INSET, dpi).min(width);
    // The row action button sits at the right end of the heading row,
    // inside the pane, on the pane color.
    let action = target.min(header_height).min(width);
    let action_rect = PixelRect {
        x: sidebar
            .x
            .saturating_add(width.saturating_sub(action).saturating_sub(dip(4, dpi))),
        y: sidebar.y.saturating_add(selector_height),
        width: action,
        height: action,
    };
    set_bounds(
        controls.header,
        PixelRect {
            x: sidebar.x.saturating_add(header_inset),
            y: sidebar.y.saturating_add(selector_height),
            width: action_rect
                .x
                .saturating_sub(sidebar.x.saturating_add(header_inset)),
            height: header_height,
        },
    );
    for (window, view) in [
        (controls.copy_anchor, SidebarMode::Outline),
        (controls.reveal, SidebarMode::Repository),
    ] {
        set_bounds(window, action_rect);
        chrome::show(window, view == mode && sidebar.width > 0 && action > 0);
    }
    let list_top = selector_height.saturating_add(header_height);
    let list = PixelRect {
        x: sidebar.x,
        y: sidebar.y.saturating_add(list_top),
        width,
        height: sidebar.height.saturating_sub(list_top),
    };
    for (window, view) in [
        (controls.outline, SidebarMode::Outline),
        (controls.tree, SidebarMode::Repository),
        (controls.recent, SidebarMode::Recent),
    ] {
        set_bounds(window, list);
        chrome::show(window, view == mode && sidebar.width > 0);
    }
    set_bounds(controls.splitter, splitter);
    chrome::show(controls.splitter, splitter.width > 0);
    // Outline and recent rows keep the 44-DIP target (the accessibility
    // gate measures outline items); tree rows are 32 DIPs (CHROME-18).
    let item = isize::try_from(target).unwrap_or(44);
    send(controls.outline, LB_SETITEMHEIGHT, 0, item);
    send(controls.recent, LB_SETITEMHEIGHT, 0, item);
    send(
        controls.tree,
        TVM_SETITEMHEIGHT,
        dip(TREE_ROW, dpi) as usize,
        0,
    );
    send(controls.tree, TVM_SETINDENT, dip(12, dpi) as usize, 0);
}

/// Room the tab control keeps beside its items (a 2-pixel lead and the
/// selected tab's 2-pixel inflation on each side), in DIPs, with margin.
const SELECTOR_SLACK: u32 = 8;

/// The width of each of the three view items in a selector `width` pixels
/// wide at `dpi`: equal shares of the pane less [`SELECTOR_SLACK`], so the
/// items always fit and the native up-down scroll control never shows
/// (owner check 2026-11-07: "Recer" and arrows at the default pane width).
pub(super) fn selector_item_width(width: u32, dpi: u32) -> u32 {
    width.saturating_sub(dip(SELECTOR_SLACK, dpi)) / 3
}

fn make_lparam(low: u32, high: u32) -> isize {
    ((high & 0xffff) << 16 | (low & 0xffff)) as isize
}

pub(super) fn select_mode(selector: HWND, mode: SidebarMode) {
    send(selector, TCM_SETCURSEL, mode.index(), 0);
}

/// Replaces the outline items. Item data holds the indent in DIPs.
pub(super) fn fill_outline(list: HWND, entries: &[OutlineEntry]) {
    send(list, LB_RESETCONTENT, 0, 0);
    for entry in entries {
        let text = wide(&entry.text);
        let index = send(list, LB_ADDSTRING, 0, text.as_ptr() as isize).0;
        if let Ok(index) = usize::try_from(index) {
            send(list, LB_SETITEMDATA, index, entry.indent_dip() as isize);
        }
    }
}

pub(super) fn fill_recent(list: HWND, paths: &[String]) {
    send(list, LB_RESETCONTENT, 0, 0);
    for path in paths {
        let text = wide(path);
        send(list, LB_ADDSTRING, 0, text.as_ptr() as isize);
    }
}

/// The selected list index, if any.
pub(super) fn list_selection(list: HWND) -> Option<usize> {
    usize::try_from(send(list, LB_GETCURSEL, 0, 0).0).ok()
}

/// Clears the tree and inserts the root with its children expanded, as
/// .NET `BuildTreeNode(tree, isRoot: true)` does. Deeper folders are filled
/// on expansion ([`insert_children`]).
pub(super) fn fill_tree(
    tree: HWND,
    generation: u32,
    root: Option<(&TreeNode, Vec<(usize, &TreeNode)>)>,
) {
    send(tree, TVM_DELETEITEM, 0, TVI_ROOT.0);
    let Some((root, children)) = root else {
        return;
    };
    let root_item = insert_item(tree, TVI_ROOT, generation, 0, root);
    if root_item.0 == 0 {
        return;
    }
    for (index, child) in children {
        insert_item(tree, root_item, generation, index, child);
    }
    send(tree, TVM_EXPAND, TVE_EXPAND, root_item.0);
}

/// Inserts the children of an expanding folder.
pub(super) fn insert_children(
    tree: HWND,
    parent: HTREEITEM,
    generation: u32,
    children: Vec<(usize, &TreeNode)>,
) {
    for (index, child) in children {
        insert_item(tree, parent, generation, index, child);
    }
}

/// An item's lParam: the tree generation in the high 32 bits and the arena
/// index in the low 32 bits, so an item of an earlier tree is recognized.
fn encode_tree_item(generation: u32, index: usize) -> LPARAM {
    let index = u32::try_from(index).unwrap_or(u32::MAX);
    LPARAM(((u64::from(generation) << 32) | u64::from(index)) as isize)
}

/// The (generation, arena index) stored by [`encode_tree_item`].
pub(super) fn decode_tree_item(data: LPARAM) -> (u32, usize) {
    let value = data.0 as u64;
    ((value >> 32) as u32, (value & 0xFFFF_FFFF) as usize)
}

fn insert_item(
    tree: HWND,
    parent: HTREEITEM,
    generation: u32,
    index: usize,
    node: &TreeNode,
) -> HTREEITEM {
    let mut text = wide(&node.name);
    let insert = TVINSERTSTRUCTW {
        hParent: parent,
        hInsertAfter: TVI_LAST,
        Anonymous: TVINSERTSTRUCTW_0 {
            item: TVITEMW {
                mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN,
                pszText: PWSTR(text.as_mut_ptr()),
                cChildren: TVITEMEXW_CHILDREN(i32::from(!node.children.is_empty())),
                lParam: encode_tree_item(generation, index),
                ..Default::default()
            },
        },
    };
    let item = send(
        tree,
        TVM_INSERTITEMW,
        0,
        std::ptr::from_ref(&insert) as isize,
    );
    HTREEITEM(item.0)
}

/// The (generation, arena index) stored in a tree item.
pub(super) fn tree_item_index(tree: HWND, item: HTREEITEM) -> Option<(u32, usize)> {
    if item.0 == 0 {
        return None;
    }
    let mut query = TVITEMW {
        mask: TVIF_PARAM,
        hItem: item,
        ..Default::default()
    };
    let found = send(
        tree,
        TVM_GETITEMW,
        0,
        std::ptr::from_mut(&mut query) as isize,
    )
    .0 != 0;
    found.then(|| decode_tree_item(query.lParam))
}

pub(super) fn tree_selection(tree: HWND) -> HTREEITEM {
    HTREEITEM(send(tree, TVM_GETNEXTITEM, TVGN_CARET, 0).0)
}

/// The item under the cursor when it was clicked on its label or icon (not
/// its expand button).
pub(super) fn tree_item_at_cursor(tree: HWND) -> HTREEITEM {
    #[repr(C)]
    struct HitTest {
        point: POINT,
        flags: u32,
        item: isize,
    }
    let mut point = POINT::default();
    // SAFETY: plain point conversion for a window on this thread.
    if unsafe { GetCursorPos(&mut point) }.is_err()
        || !unsafe { ScreenToClient(tree, &mut point) }.as_bool()
    {
        return HTREEITEM(0);
    }
    let mut hit = HitTest {
        point,
        flags: 0,
        item: 0,
    };
    send(tree, TVM_HITTEST, 0, std::ptr::from_mut(&mut hit) as isize);
    if hit.flags & TVHT_ONITEM != 0 {
        HTREEITEM(hit.item)
    } else {
        HTREEITEM(0)
    }
}

/// The text of list item `index`, from the list box (`LBS_HASSTRINGS`).
fn list_text(list: HWND, index: usize) -> Vec<u16> {
    let length = send(list, LB_GETTEXTLEN, index, 0).0;
    let Ok(length) = usize::try_from(length) else {
        return Vec::new();
    };
    let mut text = vec![0_u16; length + 1];
    let copied = send(list, LB_GETTEXT, index, text.as_mut_ptr() as isize).0;
    text.truncate(usize::try_from(copied).unwrap_or(0));
    text
}

/// The row background shared by the sidebar lists (.NET `ListViewItem`):
/// the pane color, a rounded hover or selection fill inset 4 DIPs, and a
/// 3-DIP accent pill at the left of the selected row. Returns the text
/// color for the row.
fn draw_row(item: &DRAWITEMSTRUCT, colors: SidebarColors, dpi: u32) -> ColorSpec {
    let px = |value: u32| i32::try_from(dip(value, dpi)).unwrap_or(0);
    let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
    let hot = i32::try_from(item.itemID).ok() == style::hot(item.hwndItem);
    let dc = item.hDC;
    let rect = item.rcItem;
    fill(dc, &rect, native_color(colors.background));
    let row = style::inset(&rect, px(4), px(2));
    if selected {
        style::fill_round(dc, &row, native_color(colors.selected), px(4));
        let pill_height = px(16).min(row.bottom - row.top);
        let top = row.top + (row.bottom - row.top - pill_height) / 2;
        style::fill_round(
            dc,
            &RECT {
                left: row.left,
                top,
                right: row.left + px(3),
                bottom: top + pill_height,
            },
            native_color(colors.accent),
            px(1),
        );
    } else if hot {
        style::fill_round(dc, &row, native_color(colors.hover), px(4));
    }
    if item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_NOFOCUSRECT.0 == 0 {
        // SAFETY: the DC is valid for WM_DRAWITEM.
        let _ = unsafe { DrawFocusRect(dc, &row) };
    }
    if selected {
        colors.selected_text
    } else {
        colors.text
    }
}

/// Paints one outline row: indented 14 DIPs per heading level after a
/// 16-DIP inset (the heading text's inset), hover and selection visible
/// in every theme (system colors in high contrast). Headings below level
/// two use the muted text color, so the levels read apart.
pub(super) fn draw_outline_item(
    item: &DRAWITEMSTRUCT,
    colors: SidebarColors,
    font: HFONT,
    dpi: u32,
) {
    let Ok(index) = usize::try_from(item.itemID as i32) else {
        fill(item.hDC, &item.rcItem, native_color(colors.background));
        return;
    };
    let text_color = draw_row(item, colors, dpi);
    let mut text = list_text(item.hwndItem, index);
    let indent = u32::try_from(item.itemData).unwrap_or(0);
    let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
    let foreground = if indent >= 28 && !selected {
        colors.muted
    } else {
        text_color
    };
    let rect = item.rcItem;
    let left = rect.left + i32::try_from(dip(TEXT_INSET + indent, dpi)).unwrap_or(0);
    let mut area = RECT {
        left,
        right: rect.right - i32::try_from(dip(8, dpi)).unwrap_or(0),
        ..rect
    };
    let dc = item.hDC;
    // SAFETY: the DC is valid for WM_DRAWITEM; the previous font is
    // restored before returning.
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, native_color(foreground));
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        super::style::draw_text(
            dc,
            &mut text,
            &mut area,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(dc, previous);
    }
}

/// Paints one recent file: the file name (body text) above its folder
/// (caption, muted, ellipsis in the middle of the path). The item text,
/// which is also its accessible name, stays the full path.
pub(super) fn draw_recent_item(
    item: &DRAWITEMSTRUCT,
    colors: SidebarColors,
    fonts: (HFONT, HFONT),
    dpi: u32,
) {
    const DT_PATH_ELLIPSIS: u32 = 0x4000;
    let Ok(index) = usize::try_from(item.itemID as i32) else {
        fill(item.hDC, &item.rcItem, native_color(colors.background));
        return;
    };
    let text_color = draw_row(item, colors, dpi);
    let path = String::from_utf16_lossy(&list_text(item.hwndItem, index));
    let (name, folder) = recent_parts(&path);
    let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
    let rect = item.rcItem;
    let px = |value: u32| i32::try_from(dip(value, dpi)).unwrap_or(0);
    let middle = rect.top + (rect.bottom - rect.top) / 2;
    let left = rect.left + px(TEXT_INSET);
    let right = rect.right - px(8);
    let dc = item.hDC;
    // SAFETY: the DC is valid for WM_DRAWITEM; the previous font is
    // restored before returning.
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        let previous = SelectObject(dc, HGDIOBJ(fonts.0.0));
        SetTextColor(dc, native_color(text_color));
        let mut name: Vec<u16> = name.encode_utf16().collect();
        let mut area = RECT {
            left,
            top: rect.top,
            right,
            bottom: middle + px(2),
        };
        super::style::draw_text(
            dc,
            &mut name,
            &mut area,
            DT_LEFT | DT_BOTTOM | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(dc, HGDIOBJ(fonts.1.0));
        SetTextColor(
            dc,
            native_color(if selected { text_color } else { colors.muted }),
        );
        let mut folder: Vec<u16> = folder.encode_utf16().collect();
        let mut area = RECT {
            left,
            top: middle + px(2),
            right,
            bottom: rect.bottom,
        };
        super::style::draw_text(
            dc,
            &mut folder,
            &mut area,
            DT_LEFT
                | DT_TOP
                | DT_SINGLELINE
                | DT_NOPREFIX
                | windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(DT_PATH_ELLIPSIS),
        );
        SelectObject(dc, previous);
    }
}

/// A recent file's name and its folder (the full path when it has no
/// separator).
pub(super) fn recent_parts(path: &str) -> (String, String) {
    match path.rfind(['\\', '/']) {
        Some(split) if split + 1 < path.len() => {
            let folder = &path[..split];
            // A drive root keeps its separator ("C:\").
            let folder = if folder.ends_with(':') {
                format!("{folder}\\")
            } else {
                folder.to_owned()
            };
            (path[split + 1..].to_owned(), folder)
        }
        _ => (path.to_owned(), String::new()),
    }
}

/// Gives the view selector its flat pivot look (`paint_selector`) and
/// hover tracking. The control stays a native tab control: selection,
/// keys and the UI Automation tab items are the platform's.
pub(super) fn flatten_selector(selector: HWND) {
    // SAFETY: subclasses a control of this thread; removed at WM_NCDESTROY.
    let _ = unsafe { SetWindowSubclass(selector, Some(selector_proc), SELECTOR_SUBCLASS, 0) };
    style::track_hover(selector, style::HoverKind::Tabs);
}

const SELECTOR_SUBCLASS: usize = 0x4D58_5356;

unsafe extern "system" fn selector_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match message {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            if !paint_selector(hwnd) {
                // SAFETY: default painting when the state is busy.
                return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            }
            LRESULT(0)
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            // SAFETY: default processing, then a repaint request.
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            result
        }
        WM_NCDESTROY => {
            // SAFETY: removes this procedure from its own window.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(selector_proc), SELECTOR_SUBCLASS) };
            unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
        }
        // SAFETY: default processing of the subclassed control.
        _ => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}

/// Paints the view selector as a flat pivot: the pane color, each view's
/// label centred in its tab, the selected label in the text color with a
/// 3-DIP accent underline, hover as a subtle rounded fill, and a focus
/// rectangle while keyboard cues are shown. High contrast: Window,
/// WindowText and Highlight. Returns false (nothing painted) while the
/// shell state is busy.
fn paint_selector(hwnd: HWND) -> bool {
    const TCM_GETITEMCOUNT: u32 = 0x1304;
    const TCM_GETITEMRECT: u32 = 0x130A;
    const TCM_GETCURSEL: u32 = 0x130B;
    const WM_QUERYUISTATE: u32 = 0x0129;
    const UISF_HIDEFOCUS: isize = 0x1;
    // SAFETY: the parent is the shell window on this thread.
    let Some(state) = (unsafe { GetParent(hwnd) })
        .ok()
        .and_then(|parent| unsafe { app_state_handle(parent) })
    else {
        return false;
    };
    let Some((theme, colors, font, dpi)) = state.try_borrow().ok().and_then(|state| {
        let font = state.chrome.text_fonts()?.0;
        Some((state.theme, super::palette(state.theme), font, state.dpi))
    }) else {
        return false;
    };
    drop(state);
    let sidebar = style::sidebar_colors(theme, colors);
    let indicator = style::indicator(theme, colors);
    let px = |value: u32| i32::try_from(dip(value, dpi)).unwrap_or(0);
    // Control queries first: no paint DC is open while messages are sent.
    let count = usize::try_from(send(hwnd, TCM_GETITEMCOUNT, 0, 0).0).unwrap_or(0);
    let selected = usize::try_from(send(hwnd, TCM_GETCURSEL, 0, 0).0).ok();
    let items: Vec<RECT> = (0..count)
        .map(|index| {
            let mut rect = RECT::default();
            send(
                hwnd,
                TCM_GETITEMRECT,
                index,
                std::ptr::from_mut(&mut rect) as isize,
            );
            rect
        })
        .collect();
    let cues = send(hwnd, WM_QUERYUISTATE, 0, 0).0 & UISF_HIDEFOCUS == 0;
    // SAFETY: reads this thread's focus window.
    let focused = unsafe { GetFocus() } == hwnd;
    let hot = style::hot(hwnd);
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: BeginPaint/EndPaint pair for this window's WM_PAINT.
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    // SAFETY: plain query of this thread's window.
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    fill(dc, &client, native_color(sidebar.background));
    // The divider under the pivot.
    fill(
        dc,
        &RECT {
            top: client.bottom - 1,
            ..client
        },
        native_color(sidebar.border),
    );
    // SAFETY: the paint DC is valid until EndPaint; the previous font is
    // restored below.
    let previous = unsafe {
        SetBkMode(dc, TRANSPARENT);
        SelectObject(dc, HGDIOBJ(font.0))
    };
    for (index, rect) in items.iter().enumerate() {
        let Some(mode) = SidebarMode::from_index(index) else {
            continue;
        };
        let is_selected = selected == Some(index);
        let body = RECT {
            bottom: client.bottom - 1,
            ..*rect
        };
        if hot == i32::try_from(index).ok() && !is_selected {
            style::fill_round(
                dc,
                &style::inset(&body, px(2), px(6)),
                native_color(sidebar.hover),
                px(4),
            );
        }
        // A label wider than its item (a narrow pane) uses the short form
        // ("Recent"); the tab item text, its UIA name, stays the full one.
        let room = (body.right - body.left - px(8)).max(0);
        let mut label: Vec<u16> = Vec::new();
        let mut measure = body;
        for text in [mode.label(), mode.short_label()] {
            label = text.encode_utf16().collect();
            measure = body;
            super::style::draw_text(
                dc,
                &mut label,
                &mut measure,
                DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
            );
            if measure.right - measure.left <= room {
                break;
            }
        }
        // SAFETY: as above.
        unsafe {
            SetTextColor(
                dc,
                native_color(if is_selected {
                    sidebar.text
                } else {
                    sidebar.muted
                }),
            );
            let mut area = body;
            super::style::draw_text(
                dc,
                &mut label,
                &mut area,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
        if is_selected {
            let width = (measure.right - measure.left).min(body.right - body.left);
            let center = (body.left + body.right) / 2;
            let bar = RECT {
                left: center - width / 2,
                right: center - width / 2 + width,
                top: body.bottom - px(3),
                bottom: body.bottom,
            };
            style::fill_round(dc, &bar, native_color(indicator), px(1));
            if focused && cues {
                // SAFETY: as above.
                let _ = unsafe { DrawFocusRect(dc, &style::inset(&body, px(2), px(4))) };
            }
        }
    }
    // SAFETY: restores the font and ends the paint started above.
    unsafe {
        SelectObject(dc, previous);
        let _ = EndPaint(hwnd, &paint);
    }
    true
}

/// The resize handle (.NET `SidebarResizeHandle`): drag with the pointer,
/// or focus it and use Left/Right (16 DIPs), Home (narrowest) and End
/// (widest). The width is saved when a drag ends and after each key.
unsafe extern "system" fn splitter_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the parent is the shell window on this thread; the handle
    // helper returns a scoped strong reference and no borrow.
    let parent = unsafe { GetParent(hwnd) }.ok();
    match message {
        WM_LBUTTONDOWN => {
            // SAFETY: capture for this thread's own window.
            unsafe {
                SetCapture(hwnd);
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd));
            }
            if let Some(parent) = parent {
                chrome::begin_sidebar_drag(parent);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(parent) = parent {
                chrome::drag_sidebar(parent);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // SAFETY: releases this thread's capture; WM_CAPTURECHANGED
            // finishes the drag.
            let _ = unsafe { ReleaseCapture() };
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if let Some(parent) = parent {
                chrome::end_sidebar_drag(parent);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // SAFETY: shared system cursor.
            if let Ok(cursor) = unsafe { LoadCursorW(None, IDC_SIZEWE) } {
                unsafe { SetCursor(Some(cursor)) };
            }
            LRESULT(1)
        }
        WM_KEYDOWN => match (parent, sidebar_resize_key(wparam.0 as u16)) {
            (Some(parent), Some(step)) => {
                chrome::resize_sidebar_by_key(parent, step);
                LRESULT(0)
            }
            // SAFETY: default handling for this window.
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        },
        WM_SETFOCUS | WM_KILLFOCUS => {
            // SAFETY: repaint request for this thread's own window.
            let _ = unsafe { InvalidateRect(Some(hwnd), None, true) };
            LRESULT(0)
        }
        WM_PAINT => {
            paint_splitter(hwnd, parent);
            LRESULT(0)
        }
        // SAFETY: default handling for this window.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// The pane color with a one-pixel divider on the sidebar edge; the whole handle in the accent color
/// while it has keyboard focus (Highlight in high contrast).
fn paint_splitter(hwnd: HWND, parent: Option<HWND>) {
    let colors = parent
        // SAFETY: as in `splitter_proc`.
        .and_then(|parent| unsafe { app_state_handle(parent) })
        .and_then(|state| {
            state.try_borrow().ok().map(|state| {
                let colors = super::palette(state.theme);
                (colors, style::sidebar_colors(state.theme, colors))
            })
        });
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: BeginPaint/EndPaint pair for this window's WM_PAINT.
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    if let (Some((colors, sidebar)), Ok(())) = (colors, unsafe { GetClientRect(hwnd, &mut client) })
    {
        // SAFETY: reads this thread's focus window.
        let focused = unsafe { GetFocus() } == hwnd;
        if focused {
            fill(dc, &client, native_color(colors.accent));
        } else {
            // The pane color, then the one-pixel divider between the pane
            // and the document (.NET pane `BorderThickness="0,0,1,0"`).
            fill(dc, &client, native_color(sidebar.background));
            let line = RECT {
                left: client.right - 1,
                ..client
            };
            fill(dc, &line, native_color(sidebar.border));
        }
    }
    // SAFETY: ends the paint started above.
    let _ = unsafe { EndPaint(hwnd, &paint) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Dip;

    /// The three view items always fit inside the selector, at 100, 150
    /// and 200 % and every pane width (the 220-DIP minimum, the 240-DIP
    /// compact pane without a splitter, the 280-DIP default, the 520-DIP
    /// maximum), so the tab control never shows its scroll arrows.
    #[test]
    fn the_view_items_always_fit_the_pane() {
        for dpi in [96, 120, 144, 168, 192] {
            for (pane, splitter) in [(220, true), (240, false), (280, true), (520, true)] {
                let splitter = if splitter {
                    Dip::SPLITTER_WIDTH.to_pixels(dpi)
                } else {
                    0
                };
                let width = Dip(pane).to_pixels(dpi) - splitter;
                let item = selector_item_width(width, dpi);
                // A 2-pixel lead and 2 pixels of selected-tab inflation on
                // each side are the most the control adds.
                assert!(3 * item + 6 <= width, "{pane} DIPs at {dpi} DPI");
                // Equal shares: nothing wasted beyond the slack.
                assert!(width - 3 * item <= dip(SELECTOR_SLACK, dpi) + 3);
            }
        }
    }

    /// The chevrons stay visible when the pointer leaves the tree, as in
    /// the .NET `TreeView`.
    #[test]
    fn tree_chevrons_never_fade_out() {
        assert_eq!(TREE_EXTENDED & TVS_EX_FADEINOUTEXPANDOS, 0);
        assert_ne!(TREE_EXTENDED & TVS_EX_DOUBLEBUFFER, 0);
    }
}
