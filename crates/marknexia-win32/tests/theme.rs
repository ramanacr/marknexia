use marknexia_win32::theme::{
    ColorSpec, EffectiveTheme, SystemColorRole, ThemePreference, palette, resolve_theme,
};

#[test]
fn system_preference_follows_windows_but_explicit_modes_do_not() {
    assert_eq!(
        resolve_theme(ThemePreference::System, false, false),
        EffectiveTheme::Light
    );
    assert_eq!(
        resolve_theme(ThemePreference::System, true, false),
        EffectiveTheme::Dark
    );
    assert_eq!(
        resolve_theme(ThemePreference::Light, true, false),
        EffectiveTheme::Light
    );
    assert_eq!(
        resolve_theme(ThemePreference::Dark, false, false),
        EffectiveTheme::Dark
    );
}

#[test]
fn windows_high_contrast_overrides_all_brand_preferences() {
    for preference in [
        ThemePreference::System,
        ThemePreference::Light,
        ThemePreference::Dark,
    ] {
        assert_eq!(
            resolve_theme(preference, false, true),
            EffectiveTheme::HighContrast
        );
        assert_eq!(
            resolve_theme(preference, true, true),
            EffectiveTheme::HighContrast
        );
    }
}

#[test]
fn dark_palette_uses_the_approved_metallic_radium_tokens() {
    let colors = palette(EffectiveTheme::Dark);
    assert_eq!(colors.background, ColorSpec::Rgb(0x171A1C));
    assert_eq!(colors.surface, ColorSpec::Rgb(0x24292C));
    assert_eq!(colors.elevated, ColorSpec::Rgb(0x303639));
    assert_eq!(colors.hover, ColorSpec::Rgb(0x383F42));
    assert_eq!(colors.border, ColorSpec::Rgb(0x485054));
    assert_eq!(colors.subtle_border, ColorSpec::Rgb(0x353B3E));
    assert_eq!(colors.text, ColorSpec::Rgb(0xF1F4EF));
    assert_eq!(colors.text_secondary, ColorSpec::Rgb(0xAAB2B0));
    assert_eq!(colors.text_disabled, ColorSpec::Rgb(0x707876));
    assert_eq!(colors.accent, ColorSpec::Rgb(0xB7FF3C));
    assert_eq!(colors.accent_hover, ColorSpec::Rgb(0xC9FF70));
    assert_eq!(colors.accent_pressed, ColorSpec::Rgb(0x8FD622));
    assert_eq!(colors.on_accent, ColorSpec::Rgb(0x172000));
    assert_eq!(colors.success, ColorSpec::Rgb(0x68E875));
    assert_eq!(colors.info, ColorSpec::Rgb(0x62C3FF));
    assert_eq!(colors.warning, ColorSpec::Rgb(0xFFC857));
    assert_eq!(colors.error, ColorSpec::Rgb(0xFF6577));
}

#[test]
fn light_palette_preserves_existing_marknexia_tokens() {
    let colors = palette(EffectiveTheme::Light);
    assert_eq!(colors.background, ColorSpec::Rgb(0xF8FAFC));
    assert_eq!(colors.surface, ColorSpec::Rgb(0xFFFFFF));
    assert_eq!(colors.elevated, ColorSpec::Rgb(0xF1F5F9));
    assert_eq!(colors.hover, ColorSpec::Rgb(0xE2E8F0));
    assert_eq!(colors.border, ColorSpec::Rgb(0xCBD5E1));
    assert_eq!(colors.subtle_border, ColorSpec::Rgb(0xCBD5E1));
    assert_eq!(colors.text, ColorSpec::Rgb(0x0F172A));
    assert_eq!(colors.text_secondary, ColorSpec::Rgb(0x64748B));
    assert_eq!(colors.text_disabled, ColorSpec::Rgb(0x94A3B8));
    assert_eq!(colors.accent, ColorSpec::Rgb(0x06B6D4));
    assert_eq!(colors.accent_hover, ColorSpec::Rgb(0x0891B2));
    assert_eq!(colors.accent_pressed, ColorSpec::Rgb(0x0E7490));
    assert_eq!(colors.on_accent, ColorSpec::Rgb(0x0F172A));
    assert_eq!(colors.success, ColorSpec::Rgb(0x1A7F37));
    assert_eq!(colors.info, ColorSpec::Rgb(0x2563EB));
    assert_eq!(colors.warning, ColorSpec::Rgb(0x9A6700));
    assert_eq!(colors.error, ColorSpec::Rgb(0xCF222E));
}

#[test]
fn high_contrast_uses_system_colors_not_fixed_brand_colors() {
    let colors = palette(EffectiveTheme::HighContrast);
    assert_eq!(
        colors.background,
        ColorSpec::System(SystemColorRole::Window)
    );
    assert_eq!(colors.text, ColorSpec::System(SystemColorRole::WindowText));
    assert_eq!(
        colors.surface,
        ColorSpec::System(SystemColorRole::ButtonFace)
    );
    assert_eq!(
        colors.border,
        ColorSpec::System(SystemColorRole::ButtonText)
    );
    assert_eq!(colors.hover, ColorSpec::System(SystemColorRole::Highlight));
    assert_eq!(
        colors.text_disabled,
        ColorSpec::System(SystemColorRole::GrayText)
    );
    assert_eq!(colors.accent, ColorSpec::System(SystemColorRole::Highlight));
    assert_eq!(
        colors.on_accent,
        ColorSpec::System(SystemColorRole::HighlightText)
    );
    assert!(
        colors
            .all()
            .iter()
            .all(|color| matches!(color, ColorSpec::System(_)))
    );
}
