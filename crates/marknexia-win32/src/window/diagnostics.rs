//! The diagnostics bar (.NET `DiagnosticInfoBar` and `ShowDiagnostic`): one
//! message with its severity, above the document, open until the Close
//! button dismisses it or a newer message replaces it.

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, RECT},
        Graphics::Gdi::{
            DT_CENTER, DT_SINGLELINE, DT_VCENTER, HFONT, HGDIOBJ, SelectObject, SetBkMode,
            SetTextColor, TRANSPARENT,
        },
        UI::{
            Controls::DRAWITEMSTRUCT,
            WindowsAndMessaging::{WINDOW_STYLE, WS_CHILD},
        },
    },
    core::w,
};

use marknexia_core::contracts::DiagnosticSeverity;

use super::{
    WindowError,
    chrome::{ID_DIAGNOSTIC_CLOSE, ID_DIAGNOSTIC_ICON, ID_DIAGNOSTIC_TEXT, dip, set_bounds},
    create_child, fill, native_color,
};
use crate::{
    chrome_model::DiagnosticMessage,
    layout::PixelRect,
    theme::{ColorSpec, EffectiveTheme, Palette, SystemColorRole},
};

const BS_OWNERDRAW: u32 = 0x0B;
const SS_OWNERDRAW: u32 = 0x0D;
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;
const SS_ENDELLIPSIS: u32 = 0x4000;
const WS_TABSTOP: u32 = 0x0001_0000;

#[derive(Clone, Copy)]
pub(super) struct DiagnosticsControls {
    /// Owner-drawn severity stripe and glyph (an image named by severity).
    pub icon: HWND,
    /// The message as plain static text: UIA exposes it as Text named by
    /// its window text, "Warning: …" (an owner-drawn static is an image).
    pub text: HWND,
    pub close: HWND,
}

/// Created hidden; `layout` shows them when the bar is open.
pub(super) fn create(
    parent: HWND,
    instance: HINSTANCE,
) -> Result<DiagnosticsControls, WindowError> {
    let icon = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!(""),
        WINDOW_STYLE(WS_CHILD.0 | SS_OWNERDRAW),
        ID_DIAGNOSTIC_ICON as isize,
    )?;
    let text = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!(""),
        WINDOW_STYLE(WS_CHILD.0 | SS_NOPREFIX | SS_CENTERIMAGE | SS_ENDELLIPSIS),
        ID_DIAGNOSTIC_TEXT as isize,
    )?;
    let close = create_child(
        parent,
        instance,
        w!("BUTTON"),
        w!("Close"),
        WINDOW_STYLE(WS_CHILD.0 | WS_TABSTOP | BS_OWNERDRAW),
        ID_DIAGNOSTIC_CLOSE as isize,
    )?;
    Ok(DiagnosticsControls { icon, text, close })
}

/// Severity icon, message, and a 44-DIP Close button on the right.
pub(super) fn layout(controls: &DiagnosticsControls, bar: PixelRect, dpi: u32) {
    let button = bar.height.min(dip(44, dpi)).min(bar.width);
    let icon = button.min(bar.width.saturating_sub(button));
    set_bounds(controls.icon, PixelRect { width: icon, ..bar });
    set_bounds(
        controls.text,
        PixelRect {
            x: bar.x.saturating_add(icon),
            width: bar.width.saturating_sub(button).saturating_sub(icon),
            ..bar
        },
    );
    set_bounds(
        controls.close,
        PixelRect {
            x: bar.right().saturating_sub(button),
            width: button,
            ..bar
        },
    );
}

/// Announces a new message to assistive technology: a UIA notification
/// from the message element's host provider (Narrator reads it), plus the
/// MSAA live-region event for older clients.
pub(super) fn announce(text_window: HWND, message: &str) {
    use windows::{
        Win32::UI::{
            Accessibility::{
                NotificationKind_Other, NotificationProcessing_ImportantMostRecent, NotifyWinEvent,
                UiaHostProviderFromHwnd, UiaRaiseNotificationEvent,
            },
            WindowsAndMessaging::{CHILDID_SELF, EVENT_OBJECT_LIVEREGIONCHANGED, OBJID_CLIENT},
        },
        core::BSTR,
    };
    // SAFETY: the window is this thread's own control; the provider is
    // released on drop; the BSTRs outlive the call.
    unsafe {
        if let Ok(provider) = UiaHostProviderFromHwnd(text_window) {
            let _ = UiaRaiseNotificationEvent(
                &provider,
                NotificationKind_Other,
                NotificationProcessing_ImportantMostRecent,
                &BSTR::from(message),
                &BSTR::from("MarknexiaDiagnostic"),
            );
        }
        NotifyWinEvent(
            EVENT_OBJECT_LIVEREGIONCHANGED,
            text_window,
            OBJID_CLIENT.0,
            CHILDID_SELF as i32,
        );
    }
}

/// The .NET InfoBar icons (Segoe MDL2 Assets): Info, Warning, ErrorBadge.
fn severity_glyph(severity: DiagnosticSeverity) -> char {
    match severity {
        DiagnosticSeverity::Info => '\u{E946}',
        DiagnosticSeverity::Warning => '\u{E7BA}',
        DiagnosticSeverity::Error => '\u{EA39}',
    }
}

/// The bar's background: the message static's (the dark control color,
/// otherwise the system button face, which is also the high-contrast pair).
pub(super) fn background(theme: EffectiveTheme, colors: Palette) -> ColorSpec {
    if theme == EffectiveTheme::Dark {
        colors.surface
    } else {
        ColorSpec::System(SystemColorRole::ButtonFace)
    }
}

/// Paints the severity: a stripe and the .NET glyph in the severity color.
/// The background matches the message static's (the dark control color,
/// otherwise the system button face, which is also the high-contrast pair).
pub(super) fn draw_icon(
    item: &DRAWITEMSTRUCT,
    theme: EffectiveTheme,
    colors: Palette,
    message: Option<&DiagnosticMessage>,
    glyph_font: HFONT,
    dpi: u32,
) {
    let background = background(theme, colors);
    let dc = item.hDC;
    let rect = item.rcItem;
    fill(dc, &rect, native_color(background));
    let Some(message) = message else {
        return;
    };
    let accent = match message.severity {
        DiagnosticSeverity::Info => colors.info,
        DiagnosticSeverity::Warning => colors.warning,
        DiagnosticSeverity::Error => colors.error,
    };
    // High contrast: ButtonText on ButtonFace.
    let accent = super::chrome::readable_on(background, accent);
    let stripe = i32::try_from(dip(4, dpi)).unwrap_or(4);
    fill(
        dc,
        &RECT {
            right: rect.left + stripe,
            ..rect
        },
        native_color(accent),
    );
    // SAFETY: the DC is valid for WM_DRAWITEM; the previous font is
    // restored before returning.
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        let previous = SelectObject(dc, HGDIOBJ(glyph_font.0));
        SetTextColor(dc, native_color(accent));
        let mut glyph: Vec<u16> = severity_glyph(message.severity)
            .to_string()
            .encode_utf16()
            .collect();
        let mut area = RECT {
            left: rect.left + stripe,
            ..rect
        };
        super::style::draw_text(
            dc,
            &mut glyph,
            &mut area,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(dc, previous);
    }
}
