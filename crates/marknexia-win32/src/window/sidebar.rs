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
            BeginPaint, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
            DrawFocusRect, DrawTextW, EndPaint, HFONT, HGDIOBJ, InvalidateRect, PAINTSTRUCT,
            ScreenToClient, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
        },
        UI::{
            Controls::{
                DRAWITEMSTRUCT, HTREEITEM, ODS_FOCUS, ODS_SELECTED, TCITEMW, TVI_LAST, TVI_ROOT,
                TVIF_CHILDREN, TVIF_PARAM, TVIF_TEXT, TVINSERTSTRUCTW, TVINSERTSTRUCTW_0,
                TVITEMEXW_CHILDREN, TVITEMW,
            },
            Input::KeyboardAndMouse::{GetFocus, ReleaseCapture, SetCapture},
            WindowsAndMessaging::{
                DefWindowProcW, GetClientRect, GetCursorPos, GetParent, IDC_SIZEWE, LoadCursorW,
                RegisterClassW, SetCursor, WINDOW_STYLE, WM_CAPTURECHANGED, WM_KEYDOWN,
                WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR,
                WM_SETFOCUS, WNDCLASSW, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
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
};
use crate::{
    chrome_model::{OutlineEntry, SidebarMode, TreeNode, sidebar_resize_key},
    layout::PixelRect,
    theme::Palette,
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
const TVS_HASLINES: u32 = 0x2;
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
            WS_CHILD.0
                | WS_TABSTOP.0
                | TVS_HASBUTTONS
                | TVS_HASLINES
                | TVS_LINESATROOT
                | TVS_SHOWSELALWAYS,
        ),
        ID_TREE as isize,
    )?;
    let recent = create_child(
        parent,
        instance,
        w!("LISTBOX"),
        w!("Recent Markdown files"),
        WINDOW_STYLE(list_style),
        ID_RECENT as isize,
    )?;
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
    let selector_height = sidebar.height.min(target.saturating_add(dip(4, dpi)));
    set_bounds(
        selector,
        PixelRect {
            width,
            height: selector_height,
            ..sidebar
        },
    );
    let item_width = width.saturating_sub(dip(4, dpi)) / 3;
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
    let header_inset = dip(16, dpi).min(width);
    // The row action button sits at the right end of the heading row.
    let action = target.min(header_height).min(width);
    let action_rect = PixelRect {
        x: sidebar.x.saturating_add(width.saturating_sub(action)),
        y: sidebar.y.saturating_add(selector_height),
        width: action,
        height: action,
    };
    set_bounds(
        controls.header,
        PixelRect {
            x: sidebar.x.saturating_add(header_inset),
            y: sidebar.y.saturating_add(selector_height),
            width: width.saturating_sub(header_inset).saturating_sub(action),
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
    let item = isize::try_from(target).unwrap_or(44);
    send(controls.outline, LB_SETITEMHEIGHT, 0, item);
    send(controls.recent, LB_SETITEMHEIGHT, 0, item);
    send(controls.tree, TVM_SETITEMHEIGHT, target as usize, 0);
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

/// Paints one outline row: indented by heading level, selection and focus
/// visible in every theme (system colors in high contrast).
pub(super) fn draw_outline_item(item: &DRAWITEMSTRUCT, colors: Palette, font: HFONT, dpi: u32) {
    let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
    let (background, foreground) = if selected {
        (colors.accent, colors.on_accent)
    } else {
        (
            colors.surface,
            chrome::readable_on(colors.surface, colors.text),
        )
    };
    let dc = item.hDC;
    let rect = item.rcItem;
    fill(dc, &rect, native_color(background));
    let Ok(index) = usize::try_from(item.itemID as i32) else {
        return;
    };
    let length = send(item.hwndItem, LB_GETTEXTLEN, index, 0).0;
    let Ok(length) = usize::try_from(length) else {
        return;
    };
    let mut text = vec![0_u16; length + 1];
    let copied = send(item.hwndItem, LB_GETTEXT, index, text.as_mut_ptr() as isize).0;
    text.truncate(usize::try_from(copied).unwrap_or(0));
    let indent = u32::try_from(item.itemData).unwrap_or(0);
    let left = rect.left + i32::try_from(dip(8 + indent, dpi)).unwrap_or(0);
    let mut area = RECT { left, ..rect };
    // SAFETY: the DC is valid for WM_DRAWITEM; the previous font is
    // restored before returning.
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, native_color(foreground));
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        DrawTextW(
            dc,
            &mut text,
            &mut area,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(dc, previous);
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            let _ = DrawFocusRect(dc, &rect);
        }
    }
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

/// A border line on the sidebar edge; the whole handle in the accent color
/// while it has keyboard focus (Highlight in high contrast).
fn paint_splitter(hwnd: HWND, parent: Option<HWND>) {
    let colors = parent
        // SAFETY: as in `splitter_proc`.
        .and_then(|parent| unsafe { app_state_handle(parent) })
        .and_then(|state| {
            state
                .try_borrow()
                .ok()
                .map(|state| super::palette(state.theme))
        });
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: BeginPaint/EndPaint pair for this window's WM_PAINT.
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    if let (Some(colors), Ok(())) = (colors, unsafe { GetClientRect(hwnd, &mut client) }) {
        // SAFETY: reads this thread's focus window.
        let focused = unsafe { GetFocus() } == hwnd;
        if focused {
            fill(dc, &client, native_color(colors.accent));
        } else {
            fill(dc, &client, native_color(colors.surface));
            let line = RECT {
                left: client.right - 1,
                ..client
            };
            fill(dc, &line, native_color(colors.border));
        }
    }
    // SAFETY: ends the paint started above.
    let _ = unsafe { EndPaint(hwnd, &paint) };
}
