namespace Marknexia.Core;

public static class ThemeResolution
{
    public static AppTheme ResolveEffectiveTheme(AppTheme preference, bool nativeSystemIsDark) => preference switch
    {
        AppTheme.Light => AppTheme.Light,
        AppTheme.Dark => AppTheme.Dark,
        _ => nativeSystemIsDark ? AppTheme.Dark : AppTheme.Light
    };

    public static uint ResolveWebViewBackgroundArgb(AppTheme preference, bool nativeSystemIsDark) =>
        ResolveEffectiveTheme(preference, nativeSystemIsDark) == AppTheme.Dark
            ? 0xFF171A1Cu
            : 0xFFFFFFFFu;
}
