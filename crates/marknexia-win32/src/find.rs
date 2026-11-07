//! Find in page: port of `src/Marknexia.App/MainWindow.Search.cs`.
//!
//! The rendered page counts and selects matches; the shell keeps one
//! [`DocumentSearchState`] per tab and talks to the page only through the
//! typed bridge messages `find` / `findResult` / `findUnavailable` /
//! `clearFind` (`marknexia_webview::protocol`). This module holds the
//! portable rules: the key routing, the per-tab state, and the count text.
//! The native find bar controls are in [`native`]; the shell wiring is in
//! `window.rs`.
//!
//! Differences from .NET are recorded in `compat/decisions/shell-input.md`.

use std::collections::{BTreeMap, BTreeSet};

use marknexia_core::search_state::{DocumentSearchState, SearchRequestId};

const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_F: u16 = 0x46;
const VK_F3: u16 = 0x72;

/// .NET `FindInRenderedDocumentAsync` text while the document is not ready.
pub const DOCUMENT_LOADING: &str = "Document loading\u{2026}";
/// .NET diagnostic when the bridge call fails.
pub const FIND_UNAVAILABLE: &str =
    "Find is temporarily unavailable. Wait for the document to load and try again.";

/// A find command from the keyboard or the find bar buttons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindCommand {
    /// Ctrl+F: open the find bar, or close it when it is open
    /// (.NET `ToggleSearch_Click`).
    Toggle,
    /// Enter in the find box, F3, or the "Find next match" button.
    Next,
    /// Shift+Enter in the find box, Shift+F3, or "Find previous match".
    Previous,
    /// Escape or the "Close find bar" button (.NET `CloseSearch_Click`).
    Close,
}

/// Maps a key chord to a find command. Enter counts only while the find box
/// has focus (.NET `SearchBox_KeyDown`).
#[must_use]
pub fn route_find_key(
    virtual_key: u16,
    ctrl: bool,
    shift: bool,
    alt: bool,
    in_find_box: bool,
) -> Option<FindCommand> {
    if alt {
        return None;
    }
    match virtual_key {
        VK_F if ctrl && !shift => Some(FindCommand::Toggle),
        VK_F3 if !ctrl => Some(if shift {
            FindCommand::Previous
        } else {
            FindCommand::Next
        }),
        VK_ESCAPE if !ctrl && !shift => Some(FindCommand::Close),
        VK_RETURN if in_find_box && !ctrl => Some(if shift {
            FindCommand::Previous
        } else {
            FindCommand::Next
        }),
        _ => None,
    }
}

/// The count text for a result (.NET `SearchCountText`): empty for an empty
/// query, "No matches", or "<current> of <count>".
#[must_use]
pub fn count_text(query: &str, match_count: usize, current_match: usize) -> String {
    if query.is_empty() {
        String::new()
    } else if match_count == 0 {
        "No matches".to_owned()
    } else {
        format!("{current_match} of {match_count}")
    }
}

/// What the shell must do to run a find request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FindStart {
    /// Post `find` to the tab's page with this request id.
    Post {
        request: SearchRequestId,
        query: String,
    },
    /// The document is not ready: show [`DOCUMENT_LOADING`].
    Loading,
}

/// Per-tab find state for the whole shell.
#[derive(Debug, Default)]
pub struct FindModel {
    states: BTreeMap<u64, DocumentSearchState>,
    /// Tabs whose current document finished loading (.NET
    /// `IsDocumentReady`).
    ready: BTreeSet<u64>,
}

impl FindModel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&mut self, tab_id: u64) -> &mut DocumentSearchState {
        self.states.entry(tab_id).or_default()
    }

    /// The tab's stored query, restored into the find box on tab switch.
    #[must_use]
    pub fn query(&self, tab_id: u64) -> &str {
        self.states
            .get(&tab_id)
            .map_or("", DocumentSearchState::query)
    }

    /// .NET `RefreshSearchForActiveTab`: the count text for the tab's last
    /// result, or empty while none is current.
    #[must_use]
    pub fn refresh_text(&self, tab_id: Option<u64>) -> String {
        match tab_id.and_then(|tab| self.states.get(&tab)) {
            Some(search) if search.has_current_result() && !search.query().is_empty() => {
                count_text(search.query(), search.match_count(), search.current_match())
            }
            _ => String::new(),
        }
    }

    /// .NET `SearchInputBox_TextChanged`: stores the draft query. The caller
    /// clears the count text.
    pub fn set_query(&mut self, tab_id: u64, query: &str) {
        self.state(tab_id).set_query(query);
    }

    /// .NET `FindInRenderedDocumentAsync`, up to the bridge call.
    pub fn begin(&mut self, tab_id: u64, query: &str) -> FindStart {
        let ready = self.ready.contains(&tab_id);
        let search = self.state(tab_id);
        search.set_query(query);
        let request = search.begin_request();
        if ready {
            FindStart::Post {
                request,
                query: query.to_owned(),
            }
        } else {
            FindStart::Loading
        }
    }

    /// Applies a page result. Valid results are stored for any open tab;
    /// the returned text is shown only for the active tab while the find
    /// bar is visible, as in .NET.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_result(
        &mut self,
        active: Option<u64>,
        bar_visible: bool,
        tab_id: u64,
        request: SearchRequestId,
        query: &str,
        match_count: usize,
        current_match: usize,
    ) -> Option<String> {
        let search = self.states.get_mut(&tab_id)?;
        let applied = search
            .try_apply_rendered_result(request, query, match_count, current_match)
            .unwrap_or(false);
        (applied && active == Some(tab_id) && bar_visible)
            .then(|| count_text(query, match_count, current_match))
    }

    /// The page could not run the request (.NET: a null bridge result).
    /// Returns [`DOCUMENT_LOADING`] for the active tab's current request.
    #[must_use]
    pub fn apply_unavailable(
        &self,
        active: Option<u64>,
        tab_id: u64,
        request: SearchRequestId,
    ) -> Option<String> {
        let current = self
            .states
            .get(&tab_id)
            .is_some_and(|search| search.is_current_request(request));
        (current && active == Some(tab_id)).then(|| DOCUMENT_LOADING.to_owned())
    }

    /// .NET `CloseSearch_Click` for the active tab: an empty, current result.
    pub fn close(&mut self, tab_id: u64) {
        let _ = self.state(tab_id).apply_rendered_result("", 0, 0);
    }

    /// The tab's document finished loading. Returns true when .NET would
    /// re-run the search (`NavigationCompleted` with the bar visible and a
    /// query).
    pub fn document_loaded(&mut self, tab_id: u64, active: bool, bar_visible: bool) -> bool {
        if !self.ready.insert(tab_id) {
            return false;
        }
        active && bar_visible && !self.query(tab_id).is_empty()
    }

    /// The tab's document is being replaced (.NET `NavigationStarting`):
    /// not ready, results invalid, pending requests stale.
    pub fn invalidate(&mut self, tab_id: u64) {
        self.ready.remove(&tab_id);
        if let Some(search) = self.states.get_mut(&tab_id) {
            search.invalidate_results();
        }
    }

    #[must_use]
    pub fn is_ready(&self, tab_id: u64) -> bool {
        self.ready.contains(&tab_id)
    }

    /// The tab closed.
    pub fn forget(&mut self, tab_id: u64) {
        self.states.remove(&tab_id);
        self.ready.remove(&tab_id);
    }
}

/// The find bar's native controls: label, count text and three buttons. The
/// find box itself is the shell's existing `EDIT` control.
#[cfg(windows)]
#[allow(unsafe_code)] // Child-window creation, text and drawing stay here.
pub mod native {
    use std::ffi::c_void;

    use windows::{
        Win32::{
            Foundation::{COLORREF, HINSTANCE, HWND, RECT},
            Graphics::Gdi::{
                CreateSolidBrush, DeleteObject, DrawFocusRect, FillRect, FrameRect, HGDIOBJ,
            },
            UI::{
                Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED},
                WindowsAndMessaging::{
                    BS_OWNERDRAW, CreateWindowExW, GetWindowTextLengthW, GetWindowTextW, HMENU,
                    SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, SetWindowTextW,
                    ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CHILD, WS_TABSTOP,
                },
            },
        },
        core::{Error, PCWSTR, w},
    };

    use crate::layout::{Dip, PixelRect};

    pub const LABEL_ID: isize = 1009;
    pub const COUNT_ID: isize = 1010;
    pub const PREVIOUS_ID: isize = 1006;
    pub const NEXT_ID: isize = 1007;
    pub const CLOSE_ID: isize = 1008;
    /// `EN_CHANGE` from the find box.
    pub const EN_CHANGE: u32 = 0x0300;
    /// The find box: up to 280 DIPs wide, 28 DIPs high (CHROME-17).
    const EDIT_WIDTH: Dip = Dip(280);
    const EDIT_HEIGHT: Dip = Dip(28);
    const COUNT_WIDTH: Dip = Dip(96);
    const PADDING: Dip = Dip(8);
    /// The longest text read back from the find box, in UTF-16 units. The
    /// bridge limit is `MAX_FIND_QUERY_UNITS`; the box itself is limited to
    /// it with `EM_LIMITTEXT`.
    pub const MAX_QUERY_UNITS: usize = marknexia_webview::protocol::MAX_FIND_QUERY_UNITS;

    #[derive(Clone, Copy, Debug)]
    pub struct FindControls {
        pub edit: HWND,
        pub label: HWND,
        pub count: HWND,
        pub previous: HWND,
        pub next: HWND,
        pub close: HWND,
    }

    fn child(
        parent: HWND,
        instance: HINSTANCE,
        class: PCWSTR,
        text: PCWSTR,
        style: u32,
        id: isize,
    ) -> Result<HWND, Error> {
        // SAFETY: `parent` is this thread's live shell window; the strings
        // are static; the control ID is passed as the HMENU value.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                text,
                WINDOW_STYLE(WS_CHILD.0 | style),
                0,
                0,
                0,
                0,
                Some(parent),
                Some(HMENU(id as *mut c_void)),
                Some(instance),
                None,
            )
        }
    }

    impl FindControls {
        /// Creates the find bar around the existing find box `edit`. The
        /// controls start hidden, like .NET's collapsed `SearchBarGrid`. The
        /// visible label "Find in document" precedes the box, so the box's
        /// accessible name is the .NET `AutomationProperties.Name`.
        pub fn create(parent: HWND, instance: HINSTANCE, edit: HWND) -> Result<Self, Error> {
            const EM_LIMITTEXT: u32 = 0x00C5;
            let label = child(
                parent,
                instance,
                w!("STATIC"),
                w!("Find in document"),
                0,
                LABEL_ID,
            )?;
            // SS_NOPREFIX | SS_CENTERIMAGE: one vertically centred line.
            let count = child(parent, instance, w!("STATIC"), w!(""), 0x280, COUNT_ID)?;
            let button = BS_OWNERDRAW as u32 | WS_TABSTOP.0;
            let previous = child(
                parent,
                instance,
                w!("BUTTON"),
                w!("Find previous match"),
                button,
                PREVIOUS_ID,
            )?;
            let next = child(
                parent,
                instance,
                w!("BUTTON"),
                w!("Find next match"),
                button,
                NEXT_ID,
            )?;
            let close = child(
                parent,
                instance,
                w!("BUTTON"),
                w!("Close find bar"),
                button,
                CLOSE_ID,
            )?;
            // SAFETY: all handles are live children of this thread's window.
            unsafe {
                // The box starts empty; "Find" was a placeholder caption.
                let _ = SetWindowTextW(edit, w!(""));
                let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                    edit,
                    EM_LIMITTEXT,
                    Some(windows::Win32::Foundation::WPARAM(MAX_QUERY_UNITS)),
                    None,
                );
                // Z-order: the label directly before the box (name source),
                // then the box, the count and the buttons (tab order).
                let previous_sibling = windows::Win32::UI::WindowsAndMessaging::GetWindow(
                    edit,
                    windows::Win32::UI::WindowsAndMessaging::GW_HWNDPREV,
                )
                .ok();
                let insert = |window: HWND, after: Option<HWND>| {
                    let _ = SetWindowPos(
                        window,
                        after.or(Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOP)),
                        0,
                        0,
                        0,
                        0,
                        windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                            | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                            | SWP_NOACTIVATE,
                    );
                };
                insert(label, previous_sibling);
                insert(edit, Some(label));
                insert(count, Some(edit));
                insert(previous, Some(count));
                insert(next, Some(previous));
                insert(close, Some(next));
            }
            Ok(Self {
                edit,
                label,
                count,
                previous,
                next,
                close,
            })
        }

        pub fn is_button(&self, window: HWND) -> bool {
            window == self.previous || window == self.next || window == self.close
        }

        /// Lays the bar out inside `bar` (the shell layout's find bar, the
        /// first row of the document column): a compact group at the right
        /// end, as the .NET floating search header: the box (bordered, 28
        /// DIPs high, centred), the count, then three 44-DIP buttons. The
        /// label keeps a zero-size place before the box: it names the box
        /// for UI Automation. Hidden when the bar has no height.
        pub fn layout(&self, bar: PixelRect, dpi: u32) {
            let visible = bar.height > 0 && bar.width > 0;
            let button = Dip::INTERACTION_TARGET.to_pixels(dpi).min(bar.width / 6);
            let padding = PADDING.to_pixels(dpi).min(bar.width / 8);
            let count = COUNT_WIDTH.to_pixels(dpi).min(bar.width / 4);
            let available = bar
                .width
                .saturating_sub(count)
                .saturating_sub(button.saturating_mul(3))
                .saturating_sub(padding.saturating_mul(3));
            let edit = EDIT_WIDTH.to_pixels(dpi).min(available);
            let edit_height = EDIT_HEIGHT.to_pixels(dpi).min(bar.height);
            let group = edit
                .saturating_add(padding)
                .saturating_add(count)
                .saturating_add(button.saturating_mul(3));
            let mut x = bar
                .right()
                .saturating_sub(padding)
                .saturating_sub(group)
                .max(bar.x);
            let edit_y = bar.y + bar.height.saturating_sub(edit_height) / 2;
            let place = |window: HWND, x: u32, y: u32, width: u32, height: u32| {
                // SAFETY: live child windows of this thread; no pointers.
                unsafe {
                    let _ = SetWindowPos(
                        window,
                        None,
                        x as i32,
                        y as i32,
                        width as i32,
                        height as i32,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    );
                    let _ = ShowWindow(window, if visible { SW_SHOW } else { SW_HIDE });
                }
            };
            place(self.label, x, bar.y, 0, 0);
            place(self.edit, x, edit_y, edit, edit_height);
            x = x.saturating_add(edit).saturating_add(padding);
            place(self.count, x, bar.y, count, bar.height);
            x = x.saturating_add(count);
            for window in [self.previous, self.next, self.close] {
                place(window, x, bar.y, button, bar.height);
                x = x.saturating_add(button);
            }
            if visible {
                // Inner margins of the box: 6 DIPs each side.
                const EM_SETMARGINS: u32 = 0x00D3;
                const EC_BOTH: usize = 0x3;
                let margin = Dip(6).to_pixels(dpi) as isize;
                // SAFETY: a live child of this thread; no pointers.
                let _ = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                        self.edit,
                        EM_SETMARGINS,
                        Some(windows::Win32::Foundation::WPARAM(EC_BOTH)),
                        Some(windows::Win32::Foundation::LPARAM(margin | (margin << 16))),
                    )
                };
            }
        }

        /// The find box text, at most [`MAX_QUERY_UNITS`] UTF-16 units.
        pub fn query(&self) -> String {
            read_text(self.edit)
        }

        pub fn set_count(&self, text: &str) {
            set_text(self.count, text);
        }
    }

    /// Reads a control's text, bounded by [`MAX_QUERY_UNITS`].
    pub fn read_text(window: HWND) -> String {
        // SAFETY: a live window of this thread; the buffer outlives the call.
        let length = usize::try_from(unsafe { GetWindowTextLengthW(window) }).unwrap_or(0);
        let mut buffer = vec![0_u16; length.min(MAX_QUERY_UNITS) + 1];
        let copied = unsafe { GetWindowTextW(window, &mut buffer) };
        let copied = usize::try_from(copied).unwrap_or(0).min(buffer.len());
        String::from_utf16_lossy(&buffer[..copied])
    }

    /// Sets a control's text. The caller holds no `AppState` borrow: an
    /// `EDIT` sends `EN_CHANGE` synchronously.
    pub fn set_text(window: HWND, text: &str) {
        let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `text` is NUL-terminated and outlives the call.
        let _ = unsafe { SetWindowTextW(window, PCWSTR::from_raw(text.as_ptr())) };
    }

    /// Colors for an owner-drawn find button.
    pub struct ButtonColors {
        pub face: COLORREF,
        pub pressed: COLORREF,
        pub text: COLORREF,
        pub on_pressed: COLORREF,
        pub border: COLORREF,
    }

    /// A line-drawn glyph, independent of the fonts the DC has.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Glyph {
        ChevronUp,
        ChevronDown,
        Cross,
    }

    /// Draws `glyph` centered in `rect`, `size` pixels square, with a
    /// `stroke`-pixel pen of `color`.
    pub fn draw_glyph(
        dc: windows::Win32::Graphics::Gdi::HDC,
        rect: &RECT,
        glyph: Glyph,
        color: COLORREF,
        size: i32,
        stroke: i32,
    ) {
        use windows::Win32::Graphics::Gdi::{CreatePen, LineTo, MoveToEx, PS_SOLID, SelectObject};
        let half = (size / 2).max(1);
        let cx = (rect.left + rect.right) / 2;
        let cy = (rect.top + rect.bottom) / 2;
        type Line = ((i32, i32), (i32, i32));
        let lines: &[Line] = match glyph {
            Glyph::Cross => &[
                ((-half, -half), (half + 1, half + 1)),
                ((-half, half), (half + 1, -half - 1)),
            ],
            Glyph::ChevronUp => &[
                ((-half, half / 2), (0, -half / 2)),
                ((0, -half / 2), (half + 1, half / 2 + 1)),
            ],
            Glyph::ChevronDown => &[
                ((-half, -half / 2), (0, half / 2)),
                ((0, half / 2), (half + 1, -half / 2 - 1)),
            ],
        };
        // SAFETY: `dc` is valid for the paint or draw-item message; the pen
        // is selected out and deleted before returning.
        unsafe {
            let pen = CreatePen(PS_SOLID, stroke.max(1), color);
            if pen.is_invalid() {
                return;
            }
            let previous = SelectObject(dc, HGDIOBJ(pen.0));
            for ((x0, y0), (x1, y1)) in lines {
                let _ = MoveToEx(dc, cx + x0, cy + y0, None);
                let _ = LineTo(dc, cx + x1, cy + y1);
            }
            SelectObject(dc, previous);
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }
    }

    /// Paints a find bar button: a glyph for the action, framed like the
    /// command button. The accessible name stays the window text.
    pub fn draw_button(
        controls: &FindControls,
        item: &DRAWITEMSTRUCT,
        colors: &ButtonColors,
        dpi: u32,
    ) -> bool {
        let glyph = if item.hwndItem == controls.previous {
            Glyph::ChevronUp
        } else if item.hwndItem == controls.next {
            Glyph::ChevronDown
        } else if item.hwndItem == controls.close {
            Glyph::Cross
        } else {
            return false;
        };
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let dc = item.hDC;
        let rect: RECT = item.rcItem;
        // SAFETY: the DC and rectangle come from WM_DRAWITEM and are valid
        // for this message; brushes are deleted after use.
        unsafe {
            let face = CreateSolidBrush(if pressed { colors.pressed } else { colors.face });
            if !face.is_invalid() {
                FillRect(dc, &rect, face);
                let _ = DeleteObject(HGDIOBJ(face.0));
            }
            let border = CreateSolidBrush(colors.border);
            if !border.is_invalid() {
                FrameRect(dc, &rect, border);
                let _ = DeleteObject(HGDIOBJ(border.0));
            }
            let scale = |dip: u32| (dip.saturating_mul(dpi) / 96).max(1) as i32;
            draw_glyph(
                dc,
                &rect,
                glyph,
                if pressed {
                    colors.on_pressed
                } else {
                    colors.text
                },
                scale(10),
                scale(1),
            );
            if item.itemState.0 & ODS_FOCUS.0 != 0 {
                let mut focus = rect;
                focus.left += 3;
                focus.top += 3;
                focus.right -= 3;
                focus.bottom -= 3;
                let _ = DrawFocusRect(dc, &focus);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_keys_follow_the_dotnet_bar_plus_f3() {
        assert_eq!(
            route_find_key(VK_F, true, false, false, false),
            Some(FindCommand::Toggle)
        );
        assert_eq!(route_find_key(VK_F, true, true, false, false), None);
        assert_eq!(route_find_key(VK_F, false, false, false, false), None);
        assert_eq!(
            route_find_key(VK_F3, false, false, false, false),
            Some(FindCommand::Next)
        );
        assert_eq!(
            route_find_key(VK_F3, false, true, false, true),
            Some(FindCommand::Previous)
        );
        assert_eq!(route_find_key(VK_F3, true, false, false, false), None);
        assert_eq!(
            route_find_key(VK_ESCAPE, false, false, false, false),
            Some(FindCommand::Close)
        );
        assert_eq!(route_find_key(VK_ESCAPE, false, true, false, false), None);
        assert_eq!(
            route_find_key(VK_RETURN, false, false, false, true),
            Some(FindCommand::Next)
        );
        assert_eq!(
            route_find_key(VK_RETURN, false, true, false, true),
            Some(FindCommand::Previous)
        );
        assert_eq!(route_find_key(VK_RETURN, false, false, false, false), None);
        assert_eq!(route_find_key(VK_F, true, false, true, false), None);
    }

    #[test]
    fn count_text_matches_dotnet() {
        assert_eq!(count_text("", 0, 0), "");
        assert_eq!(count_text("a", 0, 0), "No matches");
        assert_eq!(count_text("a", 4, 2), "2 of 4");
    }

    #[test]
    fn a_find_waits_for_the_document_then_posts_a_numbered_request() {
        let mut model = FindModel::new();
        assert_eq!(model.begin(1, "mark"), FindStart::Loading);
        assert!(
            !model.document_loaded(1, true, false),
            "bar hidden: no rerun"
        );
        let FindStart::Post { request, query } = model.begin(1, "mark") else {
            panic!("ready document must post");
        };
        assert_eq!(query, "mark");
        assert_eq!(
            model.apply_result(Some(1), true, 1, request, "mark", 3, 1),
            Some("1 of 3".to_owned())
        );
        assert_eq!(model.refresh_text(Some(1)), "1 of 3");
        // The same result again is stale: applying advanced the revision.
        assert_eq!(
            model.apply_result(Some(1), true, 1, request, "mark", 3, 2),
            None
        );
    }

    #[test]
    fn stale_mismatched_and_inactive_results_do_not_update_the_bar() {
        let mut model = FindModel::new();
        let _ = model.document_loaded(1, true, true);
        let _ = model.document_loaded(2, false, true);
        let FindStart::Post { request: old, .. } = model.begin(1, "a") else {
            panic!()
        };
        let FindStart::Post { request, .. } = model.begin(1, "ab") else {
            panic!()
        };
        assert_eq!(model.apply_result(Some(1), true, 1, old, "a", 1, 1), None);
        assert_eq!(
            model.apply_result(Some(1), true, 1, request, "a", 1, 1),
            None
        );
        // A valid result for an inactive tab is stored, not shown.
        let FindStart::Post { request: other, .. } = model.begin(2, "x") else {
            panic!()
        };
        assert_eq!(model.apply_result(Some(1), true, 2, other, "x", 2, 1), None);
        assert_eq!(model.refresh_text(Some(2)), "1 of 2");
        // Hidden bar: stored, not shown.
        let FindStart::Post { request, .. } = model.begin(1, "ab") else {
            panic!()
        };
        assert_eq!(
            model.apply_result(Some(1), false, 1, request, "ab", 0, 0),
            None
        );
        assert_eq!(model.refresh_text(Some(1)), "No matches");
        // A closed tab's late result is ignored.
        model.forget(2);
        assert_eq!(model.apply_result(Some(2), true, 2, other, "x", 2, 1), None);
    }

    #[test]
    fn unavailable_means_loading_only_for_the_active_current_request() {
        let mut model = FindModel::new();
        let _ = model.document_loaded(1, true, true);
        let FindStart::Post { request, .. } = model.begin(1, "a") else {
            panic!()
        };
        assert_eq!(model.apply_unavailable(Some(2), 1, request), None);
        assert_eq!(
            model.apply_unavailable(Some(1), 1, request),
            Some(DOCUMENT_LOADING.to_owned())
        );
        model.set_query(1, "b");
        assert_eq!(model.apply_unavailable(Some(1), 1, request), None);
    }

    #[test]
    fn close_and_reload_follow_dotnet() {
        let mut model = FindModel::new();
        let _ = model.document_loaded(1, true, true);
        model.set_query(1, "a");
        model.close(1);
        assert_eq!(model.query(1), "");
        assert_eq!(model.refresh_text(Some(1)), "");
        model.set_query(1, "a");
        model.invalidate(1);
        assert!(!model.is_ready(1));
        assert!(
            model.document_loaded(1, true, true),
            "visible bar with a query reruns"
        );
        assert!(!model.document_loaded(1, true, true), "only once per load");
    }
}
