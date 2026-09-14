using FluentAssertions;
using Marknexia.Core;
using Xunit;

namespace Marknexia.Core.Tests;

public sealed class ThemeResolutionTests
{
    [Theory]
    [InlineData(AppTheme.Light, false, AppTheme.Light)]
    [InlineData(AppTheme.Dark, false, AppTheme.Dark)]
    [InlineData(AppTheme.System, false, AppTheme.Light)]
    [InlineData(AppTheme.System, true, AppTheme.Dark)]
    public void ResolveEffectiveTheme_UsesExplicitPreferenceOrNativeSystemTheme(
        AppTheme preference,
        bool nativeSystemIsDark,
        AppTheme expected)
    {
        ThemeResolution.ResolveEffectiveTheme(preference, nativeSystemIsDark).Should().Be(expected);
    }

    // Would fail if the viewport fallback stopped following the shared effective-theme decision.
    [Theory]
    [InlineData(AppTheme.Light, true, 0xFFFFFFFFu)]
    [InlineData(AppTheme.Dark, false, 0xFF171A1Cu)]
    [InlineData(AppTheme.System, false, 0xFFFFFFFFu)]
    [InlineData(AppTheme.System, true, 0xFF171A1Cu)]
    public void ResolveWebViewBackgroundArgb_UsesTheEffectiveTheme(
        AppTheme preference,
        bool nativeSystemIsDark,
        uint expected)
    {
        ThemeResolution.ResolveWebViewBackgroundArgb(preference, nativeSystemIsDark).Should().Be(expected);
    }
}
