//! Shared look of the owner-drawn chrome: the system UI font faces, a GDI
//! brush cache, rounded fills, the sidebar colors, and pointer-hover
//! tracking for owner-drawn buttons, list boxes and the sidebar selector.
//!
//! Nothing here borrows the shell state: hover state lives in a
//! thread-local table, and the subclass procedure only reads control
//! geometry and requests repaints.

use std::cell::{Cell, RefCell};

use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateSolidBrush,
            DEFAULT_CHARSET, DeleteObject, FF_DONTCARE, FONT_WEIGHT, GetDC, GetStockObject,
            GetTextFaceW, HBRUSH, HDC, HFONT, HGDIOBJ, InvalidateRect, NULL_BRUSH, NULL_PEN,
            OUT_DEFAULT_PRECIS, PS_SOLID, ReleaseDC, RoundRect, SelectObject,
        },
        UI::{
            Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{WM_MOUSEMOVE, WM_NCDESTROY},
        },
    },
    core::PCWSTR,
};

use crate::theme::{ColorSpec, EffectiveTheme, Palette, SystemColorRole};

/// `WM_MOUSELEAVE`.
const WM_MOUSELEAVE: u32 = 0x02A3;
const LB_GETITEMRECT: u32 = 0x0198;
const LB_ITEMFROMPOINT: u32 = 0x01A9;
const TCM_HITTEST: u32 = 0x130D;
/// The subclass id used for hover tracking.
const HOVER_SUBCLASS: usize = 0x4D58_4856;

/// What a hover-tracked control reports as its hot part.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HoverKind {
    /// The whole control (an owner-drawn button): hot part 0.
    Button,
    /// A list box: the item under the pointer.
    List,
    /// A tab control: the tab under the pointer.
    Tabs,
}

impl HoverKind {
    fn tag(self) -> usize {
        match self {
            Self::Button => 0,
            Self::List => 1,
            Self::Tabs => 2,
        }
    }

    fn from_tag(tag: usize) -> Self {
        match tag {
            1 => Self::List,
            2 => Self::Tabs,
            _ => Self::Button,
        }
    }
}

thread_local! {
    /// Hot part by control window (`isize` handle).
    static HOT: RefCell<Vec<(isize, i32)>> = const { RefCell::new(Vec::new()) };
    /// Brushes by color, kept until the window closes (`clear_brushes`).
    static BRUSHES: RefCell<Vec<(u32, HBRUSH)>> = const { RefCell::new(Vec::new()) };
    /// The detected faces: (text, icons), once.
    static FACES: Cell<Option<(bool, bool)>> = const { Cell::new(None) };
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// The Windows 11 UI text face, else the Windows 10 one.
const TEXT_FACE_11: &str = "Segoe UI Variable Text";
const TEXT_FACE_10: &str = "Segoe UI";
/// The Windows 11 icon font, else the Windows 10 one; both have the .NET
/// `FontIcon` code points.
const ICON_FACE_11: &str = "Segoe Fluent Icons";
const ICON_FACE_10: &str = "Segoe MDL2 Assets";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn raw_font(face: &str, height: i32, weight: FONT_WEIGHT) -> HFONT {
    let face = wide(face);
    // SAFETY: plain GDI object creation; the caller owns and deletes it.
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            PCWSTR::from_raw(face.as_ptr()),
        )
    }
}

/// Whether GDI maps `face` to itself (the font is installed).
fn face_installed(face: &str) -> bool {
    let font = raw_font(face, -12, FONT_WEIGHT(400));
    if font.is_invalid() {
        return false;
    }
    let mut name = [0_u16; 64];
    // SAFETY: a screen DC, released below; the font is selected out and
    // deleted before returning.
    let length = unsafe {
        let dc = GetDC(None);
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        let length = GetTextFaceW(dc, Some(&mut name));
        SelectObject(dc, previous);
        ReleaseDC(None, dc);
        let _ = DeleteObject(HGDIOBJ(font.0));
        length
    };
    let length = usize::try_from(length).unwrap_or(0).min(name.len());
    let found = String::from_utf16_lossy(&name[..length]);
    found.trim_end_matches('\0').eq_ignore_ascii_case(face)
}

fn faces() -> (bool, bool) {
    FACES.with(|faces| {
        faces.get().unwrap_or_else(|| {
            let found = (face_installed(TEXT_FACE_11), face_installed(ICON_FACE_11));
            faces.set(Some(found));
            found
        })
    })
}

/// The UI text face: Segoe UI Variable Text when installed, else Segoe UI.
pub(super) fn text_face() -> &'static str {
    if faces().0 {
        TEXT_FACE_11
    } else {
        TEXT_FACE_10
    }
}

/// The icon face: Segoe Fluent Icons when installed, else Segoe MDL2 Assets.
pub(super) fn icon_face() -> &'static str {
    if faces().1 {
        ICON_FACE_11
    } else {
        ICON_FACE_10
    }
}

/// A font `dips` DIPs high (character height, as XAML `FontSize`) at `dpi`.
pub(super) fn font(face: &str, dips: u32, weight: FONT_WEIGHT, dpi: u32) -> HFONT {
    let pixels = i32::try_from(super::chrome::dip(dips, dpi)).unwrap_or(14);
    raw_font(face, -pixels, weight)
}

// ---------------------------------------------------------------------------
// Brushes and shapes
// ---------------------------------------------------------------------------

/// A solid brush of `color`, cached for the window's lifetime. Never
/// delete it.
pub(super) fn brush(color: COLORREF) -> HBRUSH {
    BRUSHES.with(|brushes| {
        let Ok(mut brushes) = brushes.try_borrow_mut() else {
            // SAFETY: a shared stock brush (never deleted).
            return HBRUSH(unsafe { GetStockObject(NULL_BRUSH) }.0);
        };
        if let Some((_, brush)) = brushes.iter().find(|(key, _)| *key == color.0) {
            return *brush;
        }
        // SAFETY: plain GDI object creation; deleted by `clear_brushes`.
        let brush = unsafe { CreateSolidBrush(color) };
        if !brush.is_invalid() {
            // Bounded: system colors can change, so drop the oldest beyond
            // a generous limit (callers use a brush only within a message).
            if brushes.len() >= 64 {
                let (_, old) = brushes.remove(0);
                // SAFETY: created here; no control keeps a cached brush
                // beyond the current paint.
                let _ = unsafe { DeleteObject(HGDIOBJ(old.0)) };
            }
            brushes.push((color.0, brush));
        }
        brush
    })
}

/// Deletes the cached brushes (`WM_DESTROY`).
pub(super) fn clear_brushes() {
    let brushes = BRUSHES.with(|brushes| {
        brushes
            .try_borrow_mut()
            .map(|mut brushes| std::mem::take(&mut *brushes))
            .unwrap_or_default()
    });
    for (_, brush) in brushes {
        // SAFETY: created by `brush`; the window is closing.
        let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
    }
}

/// Fills `rect` with `color`, corners rounded by `radius` pixels (0: square).
pub(super) fn fill_round(dc: HDC, rect: &RECT, color: COLORREF, radius: i32) {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    if radius <= 0 {
        // SAFETY: the DC belongs to the current paint; the brush is cached.
        unsafe {
            windows::Win32::Graphics::Gdi::FillRect(dc, rect, brush(color));
        }
        return;
    }
    // SAFETY: as above; the stock pen and the previous objects are
    // selected back before returning.
    unsafe {
        let previous_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        let previous_brush = SelectObject(dc, HGDIOBJ(brush(color).0));
        // RoundRect excludes the right and bottom edges with a null pen.
        let _ = RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            radius * 2,
            radius * 2,
        );
        SelectObject(dc, previous_brush);
        SelectObject(dc, previous_pen);
    }
}

/// A one-pixel `color` outline of `rect`, corners rounded by `radius`.
pub(super) fn frame_round(dc: HDC, rect: &RECT, color: COLORREF, radius: i32) {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    // SAFETY: the DC belongs to the current paint; the pen is deleted and
    // the previous objects are selected back.
    unsafe {
        let pen = CreatePen(PS_SOLID, 1, color);
        if pen.is_invalid() {
            return;
        }
        let previous_pen = SelectObject(dc, HGDIOBJ(pen.0));
        let previous_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let _ = RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius * 2,
            radius * 2,
        );
        SelectObject(dc, previous_brush);
        SelectObject(dc, previous_pen);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

/// `rect` shrunk by `dx` and `dy` pixels on each side.
pub(super) fn inset(rect: &RECT, dx: i32, dy: i32) -> RECT {
    RECT {
        left: rect.left + dx,
        top: rect.top + dy,
        right: (rect.right - dx).max(rect.left + dx),
        bottom: (rect.bottom - dy).max(rect.top + dy),
    }
}

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

/// The sidebar's colors (.NET `MarknexiaSurfaceSubtleBrush` pane, ListView
/// hover and selection). High contrast uses system colors only: Window
/// and WindowText, Highlight and HighlightText for the selection.
#[derive(Clone, Copy, Debug)]
pub(super) struct SidebarColors {
    pub background: ColorSpec,
    pub text: ColorSpec,
    pub muted: ColorSpec,
    pub hover: ColorSpec,
    pub selected: ColorSpec,
    pub selected_text: ColorSpec,
    /// The selection pill and the selector underline.
    pub accent: ColorSpec,
    pub border: ColorSpec,
}

pub(super) fn sidebar_colors(theme: EffectiveTheme, colors: Palette) -> SidebarColors {
    use ColorSpec::System;
    use SystemColorRole as S;
    match theme {
        EffectiveTheme::HighContrast => SidebarColors {
            background: System(S::Window),
            text: System(S::WindowText),
            muted: System(S::WindowText),
            hover: System(S::Window),
            selected: System(S::Highlight),
            selected_text: System(S::HighlightText),
            accent: System(S::Highlight),
            border: System(S::WindowText),
        },
        _ => SidebarColors {
            background: colors.elevated,
            text: colors.text,
            muted: colors.text_secondary,
            hover: colors.hover,
            selected: colors.hover,
            selected_text: colors.text,
            accent: colors.accent,
            border: colors.border,
        },
    }
}

/// The selected-tab and selector accent: the .NET primary blue in the
/// light theme (`MarknexiaPrimaryBrush` #2563EB, the cyan accent is too
/// light for a 2-DIP line), the Radium accent in the dark theme, and
/// Highlight in high contrast.
pub(super) fn indicator(theme: EffectiveTheme, colors: Palette) -> ColorSpec {
    match theme {
        EffectiveTheme::Light => colors.info,
        _ => colors.accent,
    }
}

// ---------------------------------------------------------------------------
// Hover tracking
// ---------------------------------------------------------------------------

/// The hot part of `window`: `Some(0)` for a hovered button, the hovered
/// item index of a list or tab control, `None` when the pointer is away.
pub(super) fn hot(window: HWND) -> Option<i32> {
    HOT.with(|hot| {
        hot.try_borrow().ok().and_then(|hot| {
            hot.iter()
                .find(|(key, _)| *key == window.0 as isize)
                .map(|(_, part)| *part)
        })
    })
}

fn set_hot(window: HWND, part: Option<i32>) -> Option<i32> {
    HOT.with(|hot| {
        let Ok(mut hot) = hot.try_borrow_mut() else {
            return None;
        };
        let key = window.0 as isize;
        let position = hot.iter().position(|(candidate, _)| *candidate == key);
        let previous = position.map(|index| hot[index].1);
        match (position, part) {
            (Some(index), Some(part)) => hot[index].1 = part,
            (Some(index), None) => {
                hot.remove(index);
            }
            (None, Some(part)) => hot.push((key, part)),
            (None, None) => {}
        }
        previous
    })
}

/// Tracks the pointer over `window` and repaints its hot part. Owner-drawn
/// buttons and list boxes read it back with [`hot`] when they draw.
pub(super) fn track_hover(window: HWND, kind: HoverKind) {
    // SAFETY: subclasses a control of this thread; the procedure is
    // removed at WM_NCDESTROY.
    let _ = unsafe { SetWindowSubclass(window, Some(hover_proc), HOVER_SUBCLASS, kind.tag()) };
}

fn part_at(window: HWND, kind: HoverKind, lparam: LPARAM) -> Option<i32> {
    match kind {
        HoverKind::Button => Some(0),
        HoverKind::List => {
            let result = super::chrome::send(window, LB_ITEMFROMPOINT, 0, lparam.0).0 as u32;
            // HIWORD 1: outside the client area (below the last item).
            (result >> 16 == 0).then_some((result & 0xffff) as i32)
        }
        HoverKind::Tabs => {
            #[repr(C)]
            struct HitTest {
                x: i32,
                y: i32,
                flags: u32,
            }
            let mut hit = HitTest {
                x: i32::from(lparam.0 as i16),
                y: i32::from((lparam.0 >> 16) as i16),
                flags: 0,
            };
            let index = super::chrome::send(
                window,
                TCM_HITTEST,
                0,
                std::ptr::from_mut(&mut hit) as isize,
            )
            .0;
            i32::try_from(index).ok().filter(|index| *index >= 0)
        }
    }
}

fn invalidate_part(window: HWND, kind: HoverKind, part: Option<i32>) {
    let rect = match (kind, part) {
        (HoverKind::List, Some(index)) => {
            let mut rect = RECT::default();
            let found = super::chrome::send(
                window,
                LB_GETITEMRECT,
                usize::try_from(index).unwrap_or(0),
                std::ptr::from_mut(&mut rect) as isize,
            )
            .0;
            (found != -1).then_some(rect)
        }
        (_, None) if kind == HoverKind::List => return,
        _ => None,
    };
    // SAFETY: repaint request for a control of this thread.
    let _ = unsafe { InvalidateRect(Some(window), rect.as_ref().map(std::ptr::from_ref), false) };
}

unsafe extern "system" fn hover_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    tag: usize,
) -> LRESULT {
    let kind = HoverKind::from_tag(tag);
    match message {
        WM_MOUSEMOVE => {
            let part = part_at(window, kind, lparam);
            let previous = hot(window);
            if previous != part {
                set_hot(window, part);
                invalidate_part(window, kind, previous);
                invalidate_part(window, kind, part);
            }
            if previous.is_none() {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: window,
                    dwHoverTime: 0,
                };
                // SAFETY: a live local; this thread's window.
                let _ = unsafe { TrackMouseEvent(&mut track) };
            }
        }
        WM_MOUSELEAVE => {
            let previous = set_hot(window, None);
            invalidate_part(window, kind, previous);
        }
        WM_NCDESTROY => {
            set_hot(window, None);
            // SAFETY: removes this procedure from its own window.
            let _ = unsafe { RemoveWindowSubclass(window, Some(hover_proc), HOVER_SUBCLASS) };
        }
        _ => {}
    }
    // SAFETY: default processing of the subclassed control.
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}
