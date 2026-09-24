//! Pure theme decisions for the native shell.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemePreference {
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectiveTheme {
    Light,
    Dark,
    HighContrast,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemColorRole {
    Window,
    WindowText,
    Highlight,
    HighlightText,
    ButtonFace,
    ButtonText,
    GrayText,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorSpec {
    Rgb(u32),
    System(SystemColorRole),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    pub background: ColorSpec,
    pub surface: ColorSpec,
    pub elevated: ColorSpec,
    pub hover: ColorSpec,
    pub border: ColorSpec,
    pub subtle_border: ColorSpec,
    pub text: ColorSpec,
    pub text_secondary: ColorSpec,
    pub text_disabled: ColorSpec,
    pub accent: ColorSpec,
    pub accent_hover: ColorSpec,
    pub accent_pressed: ColorSpec,
    pub on_accent: ColorSpec,
    pub success: ColorSpec,
    pub info: ColorSpec,
    pub warning: ColorSpec,
    pub error: ColorSpec,
}

impl Palette {
    pub fn all(self) -> [ColorSpec; 17] {
        [
            self.background,
            self.surface,
            self.elevated,
            self.hover,
            self.border,
            self.subtle_border,
            self.text,
            self.text_secondary,
            self.text_disabled,
            self.accent,
            self.accent_hover,
            self.accent_pressed,
            self.on_accent,
            self.success,
            self.info,
            self.warning,
            self.error,
        ]
    }
}

pub fn resolve_theme(
    preference: ThemePreference,
    os_dark: bool,
    high_contrast: bool,
) -> EffectiveTheme {
    if high_contrast {
        return EffectiveTheme::HighContrast;
    }
    match preference {
        ThemePreference::Light => EffectiveTheme::Light,
        ThemePreference::Dark => EffectiveTheme::Dark,
        ThemePreference::System if os_dark => EffectiveTheme::Dark,
        ThemePreference::System => EffectiveTheme::Light,
    }
}

pub fn palette(theme: EffectiveTheme) -> Palette {
    use ColorSpec::{Rgb, System};
    use SystemColorRole as S;

    match theme {
        EffectiveTheme::Dark => Palette {
            background: Rgb(0x171A1C),
            surface: Rgb(0x24292C),
            elevated: Rgb(0x303639),
            hover: Rgb(0x383F42),
            border: Rgb(0x485054),
            subtle_border: Rgb(0x353B3E),
            text: Rgb(0xF1F4EF),
            text_secondary: Rgb(0xAAB2B0),
            text_disabled: Rgb(0x707876),
            accent: Rgb(0xB7FF3C),
            accent_hover: Rgb(0xC9FF70),
            accent_pressed: Rgb(0x8FD622),
            on_accent: Rgb(0x172000),
            success: Rgb(0x68E875),
            info: Rgb(0x62C3FF),
            warning: Rgb(0xFFC857),
            error: Rgb(0xFF6577),
        },
        EffectiveTheme::Light => Palette {
            background: Rgb(0xF8FAFC),
            surface: Rgb(0xFFFFFF),
            elevated: Rgb(0xF1F5F9),
            hover: Rgb(0xE2E8F0),
            border: Rgb(0xCBD5E1),
            subtle_border: Rgb(0xCBD5E1),
            text: Rgb(0x0F172A),
            text_secondary: Rgb(0x64748B),
            text_disabled: Rgb(0x94A3B8),
            accent: Rgb(0x06B6D4),
            accent_hover: Rgb(0x0891B2),
            accent_pressed: Rgb(0x0E7490),
            on_accent: Rgb(0x0F172A),
            success: Rgb(0x1A7F37),
            info: Rgb(0x2563EB),
            warning: Rgb(0x9A6700),
            error: Rgb(0xCF222E),
        },
        EffectiveTheme::HighContrast => Palette {
            background: System(S::Window),
            surface: System(S::ButtonFace),
            elevated: System(S::ButtonFace),
            hover: System(S::Highlight),
            border: System(S::ButtonText),
            subtle_border: System(S::ButtonText),
            text: System(S::WindowText),
            text_secondary: System(S::WindowText),
            text_disabled: System(S::GrayText),
            accent: System(S::Highlight),
            accent_hover: System(S::Highlight),
            accent_pressed: System(S::Highlight),
            on_accent: System(S::HighlightText),
            success: System(S::WindowText),
            info: System(S::WindowText),
            warning: System(S::WindowText),
            error: System(S::WindowText),
        },
    }
}
