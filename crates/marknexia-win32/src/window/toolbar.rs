//! The command toolbar and the menu bar (.NET `MainWindow.xaml` command
//! grid and `MenuBar`). Buttons are owner-drawn `BUTTON` controls: the
//! window text is the .NET automation name, the face shows the .NET glyph
//! and label, and a tooltip shows the .NET `ToolTipService.ToolTip` text.

use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{
            CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
            DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
            DeleteObject, DrawFocusRect, DrawTextW, FF_DONTCARE, FONT_WEIGHT, FW_NORMAL,
            FW_SEMIBOLD, FrameRect, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode,
            SetTextColor, TRANSPARENT,
        },
        UI::{
            Controls::{
                DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, TOOLTIPS_CLASSW,
                TTF_IDISHWND, TTF_SUBCLASS, TTTOOLINFOW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CW_USEDEFAULT, CreateIconFromResourceEx, CreateMenu, CreatePopupMenu,
                CreateWindowExW, DI_NORMAL, DrawIconEx, HICON, HMENU, LR_DEFAULTCOLOR, MF_POPUP,
                MF_SEPARATOR, MF_STRING, SetMenu, WINDOW_STYLE, WS_CHILD, WS_EX_TOPMOST, WS_POPUP,
                WS_VISIBLE,
            },
        },
    },
    core::{PCWSTR, PWSTR, w},
};

use super::{
    COMMAND_OPEN_ID, child_style,
    chrome::{
        ChromeControls, ID_BACK, ID_BRAND_ICON, ID_BRAND_NAME, ID_BRAND_TAGLINE, ID_FIND,
        ID_FORWARD, ID_OPEN_FOLDER, ID_RELOAD, ID_THEME, ID_THEME_LABEL, ID_TOGGLE_SIDEBAR,
        MENU_ABOUT, MENU_CLOSE_TAB, MENU_FIND, MENU_HELP, MENU_OPEN_FILE, MENU_OPEN_FOLDER,
        MENU_RELOAD, MENU_REMOTE_ASSETS, MENU_SBOM, MENU_TOGGLE_SIDEBAR, MENU_TROUBLESHOOTING, dip,
        send, set_bounds, show, wide,
    },
    create_child, fill, native_color,
};
use crate::{
    layout::PixelRect,
    theme::{ColorSpec, Palette},
};
use windows::Win32::Foundation::HINSTANCE;

const BS_OWNERDRAW: u32 = 0x0B;
const CBS_DROPDOWNLIST: u32 = 0x3;
const WS_VSCROLL: u32 = 0x0020_0000;
const SS_OWNERDRAW: u32 = 0x0D;
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;
const SS_ENDELLIPSIS: u32 = 0x4000;
const CB_ADDSTRING: u32 = 0x0143;
const CB_SETITEMHEIGHT: u32 = 0x0153;
const TTS_ALWAYSTIP: u32 = 0x01;
const TTS_NOPREFIX: u32 = 0x02;
const TTM_ADDTOOLW: u32 = 0x0432;

/// The brand image (.NET `Assets/marknexia-48.png` family), drawn at
/// 28 DIPs. The larger source is used above 64 pixels.
const BRAND_PNG_64: &[u8] = include_bytes!("../../assets/marknexia-64.png");
const BRAND_PNG_128: &[u8] = include_bytes!("../../assets/marknexia-128.png");
const BRAND_ICON_DIP: u32 = 28;

/// One toolbar button: control id, glyph (Segoe MDL2 Assets, as the .NET
/// `FontIcon`), optional label, width in DIPs (with and without the label)
/// and the .NET tooltip.
struct ToolButton {
    id: usize,
    name: PCWSTR,
    glyph: char,
    label: Option<&'static str>,
    width: u32,
    tooltip: Option<&'static str>,
}

const BUTTONS: [ToolButton; 6] = [
    ToolButton {
        id: ID_TOGGLE_SIDEBAR,
        name: w!("Toggle sidebar"),
        glyph: '\u{E8A0}',
        label: None,
        width: 44,
        tooltip: Some("Toggle Sidebar"),
    },
    ToolButton {
        id: ID_BACK,
        name: w!("Back"),
        glyph: '\u{E72B}',
        label: None,
        width: 44,
        tooltip: Some("Back (Alt+Left)"),
    },
    ToolButton {
        id: ID_FORWARD,
        name: w!("Forward"),
        glyph: '\u{E72A}',
        label: None,
        width: 44,
        tooltip: Some("Forward (Alt+Right)"),
    },
    ToolButton {
        id: ID_RELOAD,
        name: w!("Reload document"),
        glyph: '\u{E72C}',
        label: None,
        width: 44,
        tooltip: Some("Reload (F5 / Ctrl+R)"),
    },
    ToolButton {
        id: ID_OPEN_FOLDER,
        name: w!("Open repository folder"),
        glyph: '\u{ED25}',
        label: Some("Folder"),
        width: 96,
        tooltip: Some("Open Folder (Ctrl+Shift+O)"),
    },
    // .NET `SearchToggleButton`, left of the theme selector.
    ToolButton {
        id: ID_FIND,
        name: w!("Find in document"),
        glyph: '\u{E721}',
        label: None,
        width: 44,
        tooltip: Some("Find in Document (Ctrl+F)"),
    },
];

/// The .NET `OpenFileButton` face: the Open glyph and the "Open" label
/// (`OpenFileLabel`). Its name is "Open Markdown file"; .NET gives it no
/// tooltip.
const OPEN_FACE: (char, Option<&str>) = ('\u{E8E5}', Some("Open"));
const OPEN_BUTTON_WIDTH: u32 = 88;
/// A labelled button collapsed to its glyph (.NET `CompactWindow`).
const ICON_WIDTH: u32 = 44;
const BRAND_ICON_WIDTH: u32 = 36;
const BRAND_DETAILS_WIDTH: u32 = 150;
/// .NET `BrandDetails` right margin (16) beyond the panel spacing.
const BRAND_MARGIN: u32 = 12;
/// .NET `AppBarSeparator` before the Open button.
const SEPARATOR: u32 = 8;
const THEME_LABEL_WIDTH: u32 = 56;
const THEME_WIDTH: u32 = 110;
/// .NET `CompactWindow`: `ThemeSelector.Width = 86`.
const COMPACT_THEME_WIDTH: u32 = 86;
const GAP: u32 = 4;
const PADDING: u32 = 8;

#[derive(Clone, Copy)]
pub(super) struct Toolbar {
    pub toggle: HWND,
    /// The brand image (an owner-drawn static named "Marknexia").
    pub brand_icon: HWND,
    /// .NET `BrandDetails`: the product name and the tagline.
    pub brand_name: HWND,
    pub brand_tagline: HWND,
    pub back: HWND,
    pub forward: HWND,
    pub reload: HWND,
    pub folder: HWND,
    pub find: HWND,
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
    // Not tab stops: the brand is decoration, as in .NET.
    let static_style = |extra: u32| WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | SS_NOPREFIX | extra);
    let brand_icon = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!("Marknexia"),
        static_style(SS_OWNERDRAW),
        ID_BRAND_ICON as isize,
    )?;
    let brand_name = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!("Marknexia"),
        static_style(SS_CENTERIMAGE | SS_ENDELLIPSIS),
        ID_BRAND_NAME as isize,
    )?;
    let brand_tagline = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!("Read. Explore. Understand."),
        static_style(SS_CENTERIMAGE | SS_ENDELLIPSIS),
        ID_BRAND_TAGLINE as isize,
    )?;
    let theme_label = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!("Theme"),
        // Not a tab stop: it only names the combo box that follows it.
        static_style(SS_CENTERIMAGE),
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
        brand_icon,
        brand_name,
        brand_tagline,
        back: buttons[1],
        forward: buttons[2],
        reload: buttons[3],
        folder: buttons[4],
        find: buttons[5],
        theme_label,
        theme,
    })
}

/// The .NET tooltips of the toolbar buttons, plus `extra` (sidebar
/// controls). One tooltip window serves every tool; it subclasses each
/// tool for its mouse messages and copies the texts. It never touches the
/// shell state. `None` when the tooltip window could not be created.
pub(super) fn create_tooltips(
    parent: HWND,
    instance: HINSTANCE,
    toolbar: &Toolbar,
    extra: &[(HWND, &str)],
) -> Option<HWND> {
    // SAFETY: creates a tooltip window owned by the shell window on this
    // thread; it is destroyed with its owner.
    let tooltip = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            PCWSTR::null(),
            WINDOW_STYLE(WS_POPUP.0 | TTS_ALWAYSTIP | TTS_NOPREFIX),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(parent),
            None,
            Some(instance),
            None,
        )
    }
    .ok()?;
    let buttons = [
        toolbar.toggle,
        toolbar.back,
        toolbar.forward,
        toolbar.reload,
        toolbar.folder,
        toolbar.find,
    ];
    let tools = buttons
        .into_iter()
        .zip(&BUTTONS)
        .filter_map(|(window, button)| button.tooltip.map(|text| (window, text)))
        .chain(extra.iter().copied());
    for (window, text) in tools {
        let mut text = wide(text);
        let info = TTTOOLINFOW {
            cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd: parent,
            uId: window.0 as usize,
            lpszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        // The tooltip copies the text during the call.
        send(tooltip, TTM_ADDTOOLW, 0, std::ptr::from_ref(&info) as isize);
    }
    Some(tooltip)
}

/// The .NET menu bar: File, View, Settings and Help. "Check for updates"
/// waits for the updater (owner gate G2).
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
        let _ = AppendMenuW(view, MF_STRING, MENU_FIND, w!("&Find in document\tCtrl+F"));
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
        let _ = AppendMenuW(
            help,
            MF_STRING,
            MENU_TROUBLESHOOTING,
            w!("&Troubleshooting && diagnostics"),
        );
        let _ = AppendMenuW(
            help,
            MF_STRING,
            MENU_SBOM,
            w!("Software &components (SBOM)"),
        );
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

/// What fits in the command bar: the .NET `CompactWindow` state collapses
/// the brand details and the Open and Folder labels and narrows the theme
/// selector. The Rust-only "Theme" label, then the brand details, also
/// give way when the wide layout does not fit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Plan {
    details: bool,
    labels: bool,
    theme_label: bool,
    theme_width: u32,
}

fn plan(compact: bool, width_dip: u32) -> Plan {
    if compact {
        return Plan {
            details: false,
            labels: false,
            theme_label: false,
            theme_width: COMPACT_THEME_WIDTH,
        };
    }
    let navigation = 4 * (ICON_WIDTH + GAP) + SEPARATOR;
    let left = PADDING
        + BRAND_ICON_WIDTH
        + GAP
        + navigation
        + OPEN_BUTTON_WIDTH
        + GAP
        + BUTTONS[4].width
        + GAP;
    let right = ICON_WIDTH + GAP + THEME_WIDTH + PADDING;
    let details = BRAND_DETAILS_WIDTH + BRAND_MARGIN;
    let label = THEME_LABEL_WIDTH;
    let (details, theme_label) = if left + details + right + label <= width_dip {
        (true, true)
    } else if left + details + right <= width_dip {
        (true, false)
    } else {
        (false, false)
    };
    Plan {
        details,
        labels: true,
        theme_label,
        theme_width: THEME_WIDTH,
    }
}

/// Lays the toolbar out in the command bar row: sidebar toggle, brand,
/// navigation buttons, Open and Folder on the left; find and the theme
/// selector on the right.
pub(super) fn layout(
    controls: &ChromeControls,
    open: HWND,
    bar: PixelRect,
    dpi: u32,
    compact: bool,
) {
    let plan = plan(compact, bar.width * 96 / dpi.max(1));
    let height = bar.height;
    let mut x = bar.x.saturating_add(dip(PADDING, dpi));
    let place = |x: &mut u32, window: HWND, width: u32, gap: u32| {
        let width = dip(width, dpi);
        set_bounds(
            window,
            PixelRect {
                x: *x,
                y: bar.y,
                width,
                height,
            },
        );
        *x = x.saturating_add(width).saturating_add(dip(gap, dpi));
    };
    let toolbar = &controls.toolbar;
    place(&mut x, toolbar.toggle, ICON_WIDTH, GAP);
    place(&mut x, toolbar.brand_icon, BRAND_ICON_WIDTH, GAP);
    show(toolbar.brand_name, plan.details);
    show(toolbar.brand_tagline, plan.details);
    if plan.details {
        let left = x;
        let width = dip(BRAND_DETAILS_WIDTH, dpi);
        // .NET: the name (15 DIPs) above the tagline (10 DIPs).
        let name_height = dip(24, dpi).min(height);
        set_bounds(
            toolbar.brand_name,
            PixelRect {
                x: left,
                y: bar.y.saturating_add(dip(3, dpi)),
                width,
                height: name_height,
            },
        );
        set_bounds(
            toolbar.brand_tagline,
            PixelRect {
                x: left,
                y: bar
                    .y
                    .saturating_add(dip(3, dpi))
                    .saturating_add(name_height),
                width,
                height: dip(15, dpi).min(height.saturating_sub(name_height)),
            },
        );
        x = x.saturating_add(dip(BRAND_DETAILS_WIDTH + BRAND_MARGIN + GAP, dpi));
    }
    place(&mut x, toolbar.back, ICON_WIDTH, GAP);
    place(&mut x, toolbar.forward, ICON_WIDTH, GAP);
    place(&mut x, toolbar.reload, ICON_WIDTH, GAP + SEPARATOR);
    let (open_width, folder_width) = if plan.labels {
        (OPEN_BUTTON_WIDTH, BUTTONS[4].width)
    } else {
        (ICON_WIDTH, ICON_WIDTH)
    };
    place(&mut x, open, open_width, GAP);
    place(&mut x, toolbar.folder, folder_width, GAP);
    let theme_width = dip(plan.theme_width, dpi);
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
    let label_width = if plan.theme_label {
        dip(THEME_LABEL_WIDTH, dpi)
    } else {
        0
    };
    let label_x = theme_x.saturating_sub(label_width);
    show(toolbar.theme_label, plan.theme_label);
    set_bounds(
        toolbar.theme_label,
        PixelRect {
            x: label_x,
            y: bar.y,
            width: label_width,
            height,
        },
    );
    // The find toggle sits left of the theme selector, as in .NET.
    let find_width = dip(BUTTONS[5].width, dpi);
    set_bounds(
        toolbar.find,
        PixelRect {
            x: label_x
                .saturating_sub(dip(GAP, dpi))
                .saturating_sub(find_width)
                .max(x),
            y: bar.y,
            width: find_width,
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

/// .NET `BrandDetails` fonts: the name (15 DIPs, SemiBold) and the tagline
/// (10 DIPs).
pub(super) fn brand_fonts(dpi: u32) -> (HFONT, HFONT) {
    (
        font_pixels(w!("Segoe UI"), 15, FW_SEMIBOLD, dpi),
        font_pixels(w!("Segoe UI"), 10, FW_NORMAL, dpi),
    )
}

fn font(face: PCWSTR, points: i32, dpi: u32) -> HFONT {
    let height = -(points * i32::try_from(dpi).unwrap_or(96) / 72);
    create_font(face, height, FW_NORMAL)
}

fn font_pixels(face: PCWSTR, dips: u32, weight: FONT_WEIGHT, dpi: u32) -> HFONT {
    create_font(face, -i32::try_from(dip(dips, dpi)).unwrap_or(10), weight)
}

fn create_font(face: PCWSTR, height: i32, weight: FONT_WEIGHT) -> HFONT {
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
            face,
        )
    }
}

/// The brand image at 28 DIPs for `dpi`, from the embedded PNG (an icon
/// resource may hold PNG data). `None` if it could not be decoded.
pub(super) fn brand_icon(dpi: u32) -> Option<HICON> {
    let size = dip(BRAND_ICON_DIP, dpi);
    let source = if size <= 64 {
        BRAND_PNG_64
    } else {
        BRAND_PNG_128
    };
    let size = i32::try_from(size).ok()?;
    // SAFETY: reads the embedded bytes during the call; the caller owns and
    // destroys the returned icon.
    unsafe { CreateIconFromResourceEx(source, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR) }.ok()
}

/// Paints the brand image centred on `background` (the shell background).
pub(super) fn draw_brand_icon(
    item: &DRAWITEMSTRUCT,
    background: ColorSpec,
    icon: Option<HICON>,
    dpi: u32,
) {
    let dc = item.hDC;
    let rect = item.rcItem;
    fill(dc, &rect, native_color(background));
    let Some(icon) = icon else {
        return;
    };
    let size = i32::try_from(dip(BRAND_ICON_DIP, dpi)).unwrap_or(28);
    let x = rect.left + (rect.right - rect.left - size) / 2;
    let y = rect.top + (rect.bottom - rect.top - size) / 2;
    // SAFETY: the DC is valid for WM_DRAWITEM and the icon is alive.
    let _ = unsafe { DrawIconEx(dc, x, y, icon, size, size, 0, None, DI_NORMAL) };
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

/// Glyph and label for an owner-drawn toolbar button id, including the
/// Open button (`ShellControls::command`). `compact` drops the labels.
pub(super) fn button_face(id: usize, compact: bool) -> Option<(char, Option<&'static str>)> {
    let face = if id == COMMAND_OPEN_ID {
        Some(OPEN_FACE)
    } else {
        BUTTONS
            .iter()
            .find(|button| button.id == id)
            .map(|button| (button.glyph, button.label))
    };
    face.map(|(glyph, label)| (glyph, label.filter(|_| !compact)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_collapses_the_brand_details_and_labels() {
        let compact = plan(true, 600);
        assert!(!compact.details && !compact.labels && !compact.theme_label);
        assert_eq!(compact.theme_width, 86);
        let wide = plan(false, 1200);
        assert!(wide.details && wide.labels && wide.theme_label);
        assert_eq!(wide.theme_width, 110);
    }

    #[test]
    fn a_crowded_wide_bar_drops_the_theme_label_then_the_details() {
        let full = plan(false, 2000);
        let mut previous = full;
        let mut seen_label_only = false;
        for width in (720..=2000).rev() {
            let current = plan(false, width);
            assert!(current.labels);
            // The label goes first; the details never return once gone.
            assert!(!current.theme_label || current.details);
            assert!(previous.details || !current.details);
            seen_label_only |= current.details && !current.theme_label;
            previous = current;
        }
        assert!(seen_label_only);
    }

    #[test]
    fn the_open_button_face_follows_dotnet() {
        assert_eq!(
            button_face(COMMAND_OPEN_ID, false),
            Some(('\u{E8E5}', Some("Open")))
        );
        assert_eq!(button_face(COMMAND_OPEN_ID, true), Some(('\u{E8E5}', None)));
        assert_eq!(button_face(ID_OPEN_FOLDER, true), Some(('\u{ED25}', None)));
        assert_eq!(button_face(ID_BACK, false), Some(('\u{E72B}', None)));
    }
}
