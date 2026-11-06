//! The diagnostics bar (.NET `DiagnosticInfoBar` and `ShowDiagnostic`): one
//! message with its severity, above the document, open until the Close
//! button dismisses it or a newer message replaces it.

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, RECT},
        Graphics::Gdi::{
            DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawTextW, HFONT,
            HGDIOBJ, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
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
    chrome::{ID_DIAGNOSTIC_CLOSE, ID_DIAGNOSTIC_TEXT, dip, set_bounds},
    create_child, fill, native_color,
};
use crate::{chrome_model::DiagnosticMessage, layout::PixelRect, theme::Palette};

const BS_OWNERDRAW: u32 = 0x0B;
const SS_OWNERDRAW: u32 = 0x0D;
const WS_TABSTOP: u32 = 0x0001_0000;

#[derive(Clone, Copy)]
pub(super) struct DiagnosticsControls {
    pub text: HWND,
    pub close: HWND,
}

/// Created hidden; `layout` shows them when the bar is open.
pub(super) fn create(
    parent: HWND,
    instance: HINSTANCE,
) -> Result<DiagnosticsControls, WindowError> {
    let text = create_child(
        parent,
        instance,
        w!("STATIC"),
        w!(""),
        WINDOW_STYLE(WS_CHILD.0 | SS_OWNERDRAW),
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
    Ok(DiagnosticsControls { text, close })
}

/// Message area on the left, a 44-DIP Close button on the right.
pub(super) fn layout(controls: &DiagnosticsControls, bar: PixelRect, dpi: u32) {
    let button = bar.height.min(dip(44, dpi)).min(bar.width);
    set_bounds(
        controls.text,
        PixelRect {
            width: bar.width.saturating_sub(button),
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

/// The .NET InfoBar icons (Segoe MDL2 Assets): Info, Warning, ErrorBadge.
fn severity_glyph(severity: DiagnosticSeverity) -> char {
    match severity {
        DiagnosticSeverity::Info => '\u{E946}',
        DiagnosticSeverity::Warning => '\u{E7BA}',
        DiagnosticSeverity::Error => '\u{EA39}',
    }
}

/// Paints the message: a severity stripe and glyph in the severity color
/// (system colors in high contrast), then the message text. The window text
/// (the accessible name) carries the severity in words.
pub(super) fn draw(
    item: &DRAWITEMSTRUCT,
    colors: Palette,
    message: Option<&DiagnosticMessage>,
    fonts: (HFONT, HFONT),
    dpi: u32,
) {
    let Some(message) = message else {
        fill(item.hDC, &item.rcItem, native_color(colors.surface));
        return;
    };
    let accent = match message.severity {
        DiagnosticSeverity::Info => colors.info,
        DiagnosticSeverity::Warning => colors.warning,
        DiagnosticSeverity::Error => colors.error,
    };
    let dc = item.hDC;
    let rect = item.rcItem;
    fill(dc, &rect, native_color(colors.elevated));
    let stripe = i32::try_from(dip(4, dpi)).unwrap_or(4);
    fill(
        dc,
        &RECT {
            right: rect.left + stripe,
            ..rect
        },
        native_color(accent),
    );
    let height = rect.bottom - rect.top;
    // SAFETY: the DC is valid for WM_DRAWITEM; the previous font is
    // restored before returning.
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        let previous = SelectObject(dc, HGDIOBJ(fonts.0.0));
        SetTextColor(dc, native_color(accent));
        let mut glyph: Vec<u16> = severity_glyph(message.severity)
            .to_string()
            .encode_utf16()
            .collect();
        let mut glyph_area = RECT {
            left: rect.left + stripe,
            right: rect.left + stripe + height,
            ..rect
        };
        DrawTextW(
            dc,
            &mut glyph,
            &mut glyph_area,
            windows::Win32::Graphics::Gdi::DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(dc, HGDIOBJ(fonts.1.0));
        SetTextColor(dc, native_color(colors.text));
        let mut text: Vec<u16> = message.message.encode_utf16().collect();
        let mut text_area = RECT {
            left: glyph_area.right,
            ..rect
        };
        DrawTextW(
            dc,
            &mut text,
            &mut text_area,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        SelectObject(dc, previous);
    }
}
