//! The dark menu bar (CHROME-2). In the dark theme the four top-level items
//! are owner-drawn on a dark bar background (`MENUINFO.hbrBack`); the light
//! and high-contrast themes keep the system menu bar and its system colors.
//! Only documented APIs are used (no `WM_UAH*` messages), so the system
//! still paints the one-pixel line below the bar and the drop-down menus.
//!
//! An owner-drawn item has no text for MSAA or UI Automation; each item's
//! data starts with an `MSAAMENUINFO` holding the system menu text ("&File"
//! and so on), from which oleacc takes the name ("File") and the access key
//! (Alt+F), as for the system items. Owner-drawn items
//! have no system mnemonics either: `menu_char` maps Alt+F, Alt+V, Alt+S and
//! Alt+H to their items.
//!
//! Painting never borrows the shell state: what it needs lives in a
//! thread-local copy, so a menu loop that runs inside an outer frame still
//! paints.

use std::cell::RefCell;

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, SIZE, WPARAM},
        Graphics::Gdi::{
            CreateFontIndirectW, CreateSolidBrush, DT_CENTER, DT_HIDEPREFIX, DT_SINGLELINE,
            DT_VCENTER, DeleteObject, GetDC, GetTextExtentPoint32W, HBRUSH, HFONT, HGDIOBJ,
            ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
        },
        UI::{
            Accessibility::{MSAA_MENU_SIG, MSAAMENUINFO},
            Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODT_MENU},
            HiDpi::SystemParametersInfoForDpi,
            WindowsAndMessaging::{
                DrawMenuBar, HMENU, MENUINFO, MENUITEMINFOW, MFT_OWNERDRAW, MFT_STRING, MIIM_DATA,
                MIIM_FTYPE, MIIM_STRING, MIM_BACKGROUND, NONCLIENTMETRICSW,
                SPI_GETNONCLIENTMETRICS, SetMenuInfo, SetMenuItemInfoW,
            },
        },
    },
    core::{PCWSTR, PWSTR, w},
};

use super::{fill, native_color};
use crate::theme::{EffectiveTheme, Palette, palette};

const ODS_SELECTED: u32 = 0x0001;
const ODS_GRAYED: u32 = 0x0002;
const ODS_DISABLED: u32 = 0x0004;
const ODS_HOTLIGHT: u32 = 0x0040;
const ODS_INACTIVE: u32 = 0x0080;
const ODS_NOACCEL: u32 = 0x0100;
const MNC_EXECUTE: isize = 2;

/// One top-level item's data. `msaa` must come first: oleacc reads an
/// `MSAAMENUINFO` at the start of an owner-drawn item's data.
#[repr(C)]
struct BarItem {
    msaa: MSAAMENUINFO,
    /// The system menu text, with its mnemonic marker.
    label: PCWSTR,
    /// The upper-case mnemonic character.
    mnemonic: u16,
}

struct BarItems([BarItem; 4]);

// SAFETY: the pointers refer to static, immutable UTF-16 literals.
unsafe impl Sync for BarItems {}

/// `length` counts the UTF-16 units of `label` without the terminator.
const fn item(length: u32, label: PCWSTR, mnemonic: u8) -> BarItem {
    BarItem {
        msaa: MSAAMENUINFO {
            dwMSAASignature: MSAA_MENU_SIG as u32,
            cchWText: length,
            pszWText: PWSTR(label.0.cast_mut()),
        },
        label,
        mnemonic: mnemonic as u16,
    }
}

/// The top-level items in `toolbar::create_menu` order.
static ITEMS: BarItems = BarItems([
    item(5, w!("&File"), b'F'),
    item(5, w!("&View"), b'V'),
    item(9, w!("&Settings"), b'S'),
    item(5, w!("&Help"), b'H'),
]);

/// GDI objects and colors for painting the dark bar, deleted on drop.
struct MenuPaint {
    colors: Palette,
    font: HFONT,
    background: HBRUSH,
}

impl Drop for MenuPaint {
    fn drop(&mut self) {
        // SAFETY: both objects were created for this struct; the bar has
        // been given the replacement brush (or none) before the drop.
        unsafe {
            if !self.font.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.font.0));
            }
            if !self.background.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.background.0));
            }
        }
    }
}

thread_local! {
    static PAINT: RefCell<Option<MenuPaint>> = const { RefCell::new(None) };
}

/// The system menu font for `dpi`.
fn menu_font(dpi: u32) -> Option<HFONT> {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct is a live local of the size given.
    unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some(std::ptr::from_mut(&mut metrics).cast()),
            0,
            dpi,
        )
        .ok()?;
        let font = CreateFontIndirectW(&metrics.lfMenuFont);
        (!font.is_invalid()).then_some(font)
    }
}

impl MenuPaint {
    fn new(dpi: u32) -> Option<Self> {
        let colors = palette(EffectiveTheme::Dark);
        let font = menu_font(dpi)?;
        // SAFETY: plain GDI object creation; owned by the result.
        let background = unsafe { CreateSolidBrush(native_color(colors.surface)) };
        let paint = Self {
            colors,
            font,
            background,
        };
        (!background.is_invalid()).then_some(paint)
    }
}

/// Applies the menu bar style for `theme` at `dpi`: owner-drawn items on a
/// dark background in the dark theme, system items otherwise. Called on
/// theme and DPI changes, with no shell state borrowed.
pub(super) fn apply(window: HWND, menu: HMENU, theme: EffectiveTheme, dpi: u32) {
    let paint = if theme == EffectiveTheme::Dark {
        MenuPaint::new(dpi)
    } else {
        None
    };
    let dark = paint.is_some();
    let background = paint
        .as_ref()
        .map_or(HBRUSH::default(), |paint| paint.background);
    // The new objects are installed first, so measuring and painting during
    // the changes below find them; the previous ones stay alive until the
    // bar no longer uses them. (Without the slot, nothing changes.)
    let Some(previous) = PAINT.with(|slot| {
        slot.try_borrow_mut()
            .ok()
            .map(|mut slot| std::mem::replace(&mut *slot, paint))
    }) else {
        return;
    };
    for (position, item) in ITEMS.0.iter().enumerate() {
        let info = if dark {
            MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_DATA,
                fType: MFT_OWNERDRAW,
                dwItemData: std::ptr::from_ref(item) as usize,
                ..Default::default()
            }
        } else {
            MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_DATA | MIIM_STRING,
                fType: MFT_STRING,
                dwItemData: 0,
                dwTypeData: PWSTR(item.label.0.cast_mut()),
                ..Default::default()
            }
        };
        // SAFETY: `menu` is the shell's menu bar on this thread; the system
        // copies the text; the item data is static.
        let _ = unsafe { SetMenuItemInfoW(menu, position as u32, true, &info) };
    }
    let info = MENUINFO {
        cbSize: std::mem::size_of::<MENUINFO>() as u32,
        fMask: MIM_BACKGROUND,
        hbrBack: background,
        ..Default::default()
    };
    // SAFETY: as above; the brush stays alive in `PAINT` while it is set.
    let _ = unsafe { SetMenuInfo(menu, &info) };
    // SAFETY: redraws this thread's window's menu bar.
    let _ = unsafe { DrawMenuBar(window) };
    drop(previous);
}

/// The bar item an owner-drawn menu item's data refers to, if it is ours.
fn bar_item(data: usize) -> Option<(usize, &'static BarItem)> {
    ITEMS
        .0
        .iter()
        .enumerate()
        .find(|(_, item)| std::ptr::from_ref(*item) as usize == data)
}

fn text_without_marker(label: PCWSTR) -> Vec<u16> {
    // SAFETY: the labels are static NUL-terminated literals.
    let text = unsafe { label.as_wide() };
    text.iter()
        .copied()
        .filter(|unit| *unit != u16::from(b'&'))
        .collect()
}

/// `WM_MEASUREITEM` for the owner-drawn bar items.
pub(super) fn measure(window: HWND, item: &mut MEASUREITEMSTRUCT) -> bool {
    if item.CtlType != ODT_MENU {
        return false;
    }
    let Some((_, bar)) = bar_item(item.itemData) else {
        return false;
    };
    let Some(font) = PAINT.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|paint| paint.as_ref().map(|paint| paint.font))
    }) else {
        return false;
    };
    let text = text_without_marker(bar.label);
    let mut size = SIZE::default();
    // SAFETY: a screen DC of this thread's window, released below; the
    // previous font is selected back.
    unsafe {
        let dc = GetDC(Some(window));
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        let _ = GetTextExtentPoint32W(dc, &text, &mut size);
        SelectObject(dc, previous);
        ReleaseDC(Some(window), dc);
    }
    // Padding on both sides, like the system bar items.
    item.itemWidth = u32::try_from(size.cx + size.cy).unwrap_or(48);
    item.itemHeight = u32::try_from(size.cy).unwrap_or(16);
    true
}

/// `WM_DRAWITEM` for the owner-drawn bar items.
pub(super) fn draw(item: &DRAWITEMSTRUCT) -> bool {
    if item.CtlType != ODT_MENU {
        return false;
    }
    let Some((_, bar)) = bar_item(item.itemData) else {
        return false;
    };
    let Some((colors, font)) = PAINT.with(|slot| {
        slot.try_borrow()
            .ok()
            .and_then(|paint| paint.as_ref().map(|paint| (paint.colors, paint.font)))
    }) else {
        return false;
    };
    let state = item.itemState.0;
    let hot = state & (ODS_SELECTED | ODS_HOTLIGHT) != 0;
    let disabled = state & (ODS_GRAYED | ODS_DISABLED | ODS_INACTIVE) != 0;
    let background = if hot { colors.hover } else { colors.surface };
    let text_color = if disabled {
        colors.text_disabled
    } else {
        colors.text
    };
    let dc = item.hDC;
    let mut rect = item.rcItem;
    fill(dc, &rect, native_color(background));
    // SAFETY: the label is a static literal; it is copied to a local buffer
    // for DrawTextW.
    let mut text: Vec<u16> = unsafe { bar.label.as_wide() }.to_vec();
    let mut format = DT_CENTER | DT_VCENTER | DT_SINGLELINE;
    if state & ODS_NOACCEL != 0 {
        format |= DT_HIDEPREFIX;
    }
    // SAFETY: the DC is valid for WM_DRAWITEM; the previous font is
    // selected back.
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, native_color(text_color));
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        super::style::draw_text(dc, &mut text, &mut rect, format);
        SelectObject(dc, previous);
    }
    true
}

/// `WM_MENUCHAR`: Alt plus a bar item's mnemonic opens that item while the
/// items are owner-drawn.
pub(super) fn menu_char(menu: Option<HMENU>, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let menu = menu?;
    if lparam.0 != menu.0 as isize {
        return None;
    }
    let active = PAINT.with(|slot| slot.try_borrow().ok().is_some_and(|paint| paint.is_some()));
    if !active {
        return None;
    }
    let character = (wparam.0 & 0xffff) as u16;
    let upper = if (u16::from(b'a')..=u16::from(b'z')).contains(&character) {
        character - 32
    } else {
        character
    };
    ITEMS
        .0
        .iter()
        .position(|item| item.mnemonic == upper)
        .map(|position| LRESULT((MNC_EXECUTE << 16) | position as isize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bar_item_names_itself_for_msaa_with_its_access_key() {
        for item in &ITEMS.0 {
            assert_eq!(item.msaa.dwMSAASignature, 0xAA0D_F00D);
            // SAFETY: static literals.
            let pointer = PCWSTR(item.msaa.pszWText.0);
            let text = unsafe { pointer.as_wide() };
            assert_eq!(text.len(), item.msaa.cchWText as usize);
            // The access-key marker stays, as in the system menu text.
            assert_eq!(text[0], u16::from(b'&'));
            assert_eq!(text[1], item.mnemonic);
            assert_eq!(text_without_marker(item.label), &text[1..]);
        }
    }

    #[test]
    fn foreign_item_data_is_not_ours() {
        assert!(bar_item(0).is_none());
        assert!(bar_item(1).is_none());
        let first = std::ptr::from_ref(&ITEMS.0[0]) as usize;
        assert_eq!(bar_item(first).map(|(index, _)| index), Some(0));
    }
}
