//! The command toolbar and the menu bar (.NET `MainWindow.xaml` command
//! grid and `MenuBar`). Buttons are owner-drawn `BUTTON` controls: the
//! window text is the .NET automation name, the face shows the .NET glyph.

use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{
            CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
            DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
            DeleteObject, DrawFocusRect, DrawTextW, FF_DONTCARE, FW_NORMAL, FrameRect, HFONT,
            HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
        },
        UI::{
            Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED},
            WindowsAndMessaging::{
                AppendMenuW, CreateMenu, CreatePopupMenu, HMENU, MF_POPUP, MF_SEPARATOR, MF_STRING,
                SetMenu,
            },
        },
    },
    core::{PCWSTR, w},
};

use super::{
    child_style,
    chrome::{
        ChromeControls, ID_BACK, ID_FORWARD, ID_OPEN_FOLDER, ID_RELOAD, ID_THEME, ID_THEME_LABEL,
        ID_TOGGLE_SIDEBAR, MENU_ABOUT, MENU_CLOSE_TAB, MENU_HELP, MENU_OPEN_FILE, MENU_OPEN_FOLDER,
        MENU_RELOAD, MENU_REMOTE_ASSETS, MENU_TOGGLE_SIDEBAR, dip, send, set_bounds,
    },
    create_child, fill, native_color,
};
use crate::{layout::PixelRect, theme::Palette};
use windows::Win32::Foundation::HINSTANCE;

const BS_OWNERDRAW: u32 = 0x0B;
const CBS_DROPDOWNLIST: u32 = 0x3;
const WS_VSCROLL: u32 = 0x0020_0000;
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;
const CB_ADDSTRING: u32 = 0x0143;
const CB_SETITEMHEIGHT: u32 = 0x0153;

/// One toolbar button: control id, glyph (Segoe MDL2 Assets, as the .NET
/// `FontIcon`), optional label and width in DIPs.
struct ToolButton {
    id: usize,
    name: PCWSTR,
    glyph: char,
    label: Option<&'static str>,
    width: u32,
}

const BUTTONS: [ToolButton; 5] = [
    ToolButton {
        id: ID_TOGGLE_SIDEBAR,
        name: w!("Toggle sidebar"),
        glyph: '\u{E8A0}',
        label: None,
        width: 44,
    },
    ToolButton {
        id: ID_BACK,
        name: w!("Back"),
        glyph: '\u{E72B}',
        label: None,
        width: 44,
    },
    ToolButton {
        id: ID_FORWARD,
        name: w!("Forward"),
        glyph: '\u{E72A}',
        label: None,
        width: 44,
    },
    ToolButton {
        id: ID_RELOAD,
        name: w!("Reload document"),
        glyph: '\u{E72C}',
        label: None,
        width: 44,
    },
    ToolButton {
        id: ID_OPEN_FOLDER,
        name: w!("Open repository folder"),
        glyph: '\u{ED25}',
        label: Some("Folder"),
        width: 104,
    },
];

/// Width of the existing "Open Markdown" button (`ShellControls::command`).
const OPEN_BUTTON_WIDTH: u32 = 150;
const THEME_LABEL_WIDTH: u32 = 56;
const THEME_WIDTH: u32 = 110;
const GAP: u32 = 4;
const PADDING: u32 = 8;

#[derive(Clone, Copy)]
pub(super) struct Toolbar {
    pub toggle: HWND,
    pub back: HWND,
    pub forward: HWND,
    pub reload: HWND,
    pub folder: HWND,
    pub theme_label: HWND,
    pub theme: HWND,
}

pub(super) fn create(parent: HWND, instance: HINSTANCE) -> Result<Toolbar, super::WindowError> {
    let mut buttons = Vec::with_capacity(BUTTONS.len());
    for button in &BUTTONS {
        buttons.push(create_child(
            parent,
            instance,
            w!("BUTTON"),
            button.name,
            child_style(BS_OWNERDRAW),
            button.id as isize,
        )?);
    }
    let theme_label = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!("Theme"),
        // Not a tab stop: it only names the combo box that follows it.
        windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
            windows::Win32::UI::WindowsAndMessaging::WS_CHILD.0
                | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE.0
                | SS_NOPREFIX
                | SS_CENTERIMAGE,
        ),
        ID_THEME_LABEL as isize,
    )?;
    let theme = create_child(
        parent,
        instance,
        w!("COMBOBOX"),
        w!("Theme"),
        child_style(CBS_DROPDOWNLIST | WS_VSCROLL),
        ID_THEME as isize,
    )?;
    for item in [w!("System"), w!("Light"), w!("Dark")] {
        send(theme, CB_ADDSTRING, 0, item.as_ptr() as isize);
    }
    Ok(Toolbar {
        toggle: buttons[0],
        back: buttons[1],
        forward: buttons[2],
        reload: buttons[3],
        folder: buttons[4],
        theme_label,
        theme,
    })
}

/// The .NET menu bar: File, View, Settings and Help.
pub(super) fn create_menu(parent: HWND) -> Option<HMENU> {
    // SAFETY: menus are created and attached on the UI thread; a menu
    // attached with SetMenu is destroyed with its window.
    unsafe {
        let bar = CreateMenu().ok()?;
        let file = CreatePopupMenu().ok()?;
        let _ = AppendMenuW(
            file,
            MF_STRING,
            MENU_OPEN_FILE,
            w!("&Open Markdown file\tCtrl+O"),
        );
        let _ = AppendMenuW(
            file,
            MF_STRING,
            MENU_OPEN_FOLDER,
            w!("Open &repository\tCtrl+Shift+O"),
        );
        let _ = AppendMenuW(file, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(file, MF_STRING, MENU_CLOSE_TAB, w!("&Close tab\tCtrl+W"));
        let view = CreatePopupMenu().ok()?;
        let _ = AppendMenuW(view, MF_STRING, MENU_TOGGLE_SIDEBAR, w!("Toggle &sidebar"));
        let _ = AppendMenuW(view, MF_STRING, MENU_RELOAD, w!("&Reload document\tF5"));
        let settings = CreatePopupMenu().ok()?;
        let _ = AppendMenuW(
            settings,
            MF_STRING,
            MENU_REMOTE_ASSETS,
            w!("Allow remote &images (network)"),
        );
        let help = CreatePopupMenu().ok()?;
        let _ = AppendMenuW(help, MF_STRING, MENU_HELP, w!("&Keyboard shortcuts"));
        let _ = AppendMenuW(help, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(help, MF_STRING, MENU_ABOUT, w!("&About Marknexia"));
        for (popup, title) in [
            (file, w!("&File")),
            (view, w!("&View")),
            (settings, w!("&Settings")),
            (help, w!("&Help")),
        ] {
            let _ = AppendMenuW(bar, MF_POPUP, popup.0 as usize, title);
        }
        SetMenu(parent, Some(bar)).ok()?;
        Some(bar)
    }
}

/// Lays the toolbar out in the command bar row: navigation buttons, Open,
/// Folder on the left; the theme selector on the right.
pub(super) fn layout(controls: &ChromeControls, open: HWND, bar: PixelRect, dpi: u32) {
    let height = bar.height;
    let mut x = bar.x.saturating_add(dip(PADDING, dpi));
    let mut place = |window: HWND, width: u32| {
        let width = dip(width, dpi);
        set_bounds(
            window,
            PixelRect {
                x,
                y: bar.y,
                width,
                height,
            },
        );
        x = x.saturating_add(width).saturating_add(dip(GAP, dpi));
    };
    let toolbar = &controls.toolbar;
    for (window, button) in [
        toolbar.toggle,
        toolbar.back,
        toolbar.forward,
        toolbar.reload,
    ]
    .into_iter()
    .zip(&BUTTONS)
    {
        place(window, button.width);
    }
    place(open, OPEN_BUTTON_WIDTH);
    place(toolbar.folder, BUTTONS[4].width);
    let theme_width = dip(THEME_WIDTH, dpi);
    let theme_x = bar
        .right()
        .saturating_sub(dip(PADDING, dpi))
        .saturating_sub(theme_width)
        .max(x);
    set_bounds(
        toolbar.theme,
        PixelRect {
            x: theme_x,
            y: bar.y,
            width: theme_width,
            // The drop-down list extends below the selection field.
            height: height.saturating_mul(4),
        },
    );
    let label_width = dip(THEME_LABEL_WIDTH, dpi);
    set_bounds(
        toolbar.theme_label,
        PixelRect {
            x: theme_x.saturating_sub(label_width),
            y: bar.y,
            width: label_width,
            height,
        },
    );
    // The selection field and list items meet the 44-DIP target; the item
    // height excludes the field's borders.
    let field = isize::try_from(height.saturating_sub(6)).unwrap_or(0);
    send(toolbar.theme, CB_SETITEMHEIGHT, usize::MAX, field);
    send(
        toolbar.theme,
        CB_SETITEMHEIGHT,
        0,
        isize::try_from(height).unwrap_or(0),
    );
}

/// Glyph font for button faces at `dpi`.
pub(super) fn glyph_font(dpi: u32) -> HFONT {
    font(w!("Segoe MDL2 Assets"), 11, dpi)
}

/// Text font for chrome controls at `dpi` (Segoe UI 9 pt).
pub(super) fn text_font(dpi: u32) -> HFONT {
    font(w!("Segoe UI"), 9, dpi)
}

fn font(face: PCWSTR, points: i32, dpi: u32) -> HFONT {
    let height = -(points * i32::try_from(dpi).unwrap_or(96) / 72);
    // SAFETY: plain GDI object creation; the caller owns and deletes it.
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            face,
        )
    }
}

/// Paints one of the toolbar buttons (or the diagnostics close button):
/// glyph, optional label, focus rectangle; light, dark and high-contrast
/// colors from the palette.
pub(super) fn draw_button(
    item: &DRAWITEMSTRUCT,
    colors: Palette,
    glyph: char,
    label: Option<&str>,
    fonts: (HFONT, HFONT),
) {
    let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
    let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
    let (face, text) = if pressed {
        (colors.accent_pressed, colors.on_accent)
    } else if disabled {
        (colors.surface, colors.text_disabled)
    } else {
        (colors.elevated, colors.text)
    };
    // High contrast: ButtonText (or GrayText) on ButtonFace.
    let text = super::chrome::readable_on(face, text);
    let dc = item.hDC;
    let rect = item.rcItem;
    fill(dc, &rect, native_color(face));
    // SAFETY: the DC and rectangle come from WM_DRAWITEM and are valid for
    // this message; created brushes are deleted and the previous font is
    // selected back.
    unsafe {
        let border = CreateSolidBrush(native_color(colors.border));
        if !border.is_invalid() {
            FrameRect(dc, &rect, border);
            let _ = DeleteObject(HGDIOBJ(border.0));
        }
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, native_color(text));
        let previous = SelectObject(dc, HGDIOBJ(fonts.0.0));
        let mut glyph_text: Vec<u16> = glyph.to_string().encode_utf16().collect();
        match label {
            None => {
                let mut area = rect;
                DrawTextW(
                    dc,
                    &mut glyph_text,
                    &mut area,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
            }
            Some(label) => {
                let height = rect.bottom - rect.top;
                let mut glyph_area = RECT {
                    right: rect.left + height,
                    ..rect
                };
                DrawTextW(
                    dc,
                    &mut glyph_text,
                    &mut glyph_area,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
                SelectObject(dc, HGDIOBJ(fonts.1.0));
                let mut text_area = RECT {
                    left: glyph_area.right,
                    ..rect
                };
                let mut label: Vec<u16> = label.encode_utf16().collect();
                DrawTextW(
                    dc,
                    &mut label,
                    &mut text_area,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
                );
            }
        }
        SelectObject(dc, previous);
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            let mut focus = rect;
            focus.left += 3;
            focus.top += 3;
            focus.right -= 3;
            focus.bottom -= 3;
            let _ = DrawFocusRect(dc, &focus);
        }
    }
}

/// Glyph and label for an owner-drawn toolbar button id.
pub(super) fn button_face(id: usize) -> Option<(char, Option<&'static str>)> {
    BUTTONS
        .iter()
        .find(|button| button.id == id)
        .map(|button| (button.glyph, button.label))
}
