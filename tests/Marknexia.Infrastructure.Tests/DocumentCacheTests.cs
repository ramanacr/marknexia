using FluentAssertions;
using Marknexia.Core;
using Marknexia.Infrastructure;
using Xunit;

namespace Marknexia.Infrastructure.Tests;

public sealed class DocumentCacheTests
{
    [Fact]
    public void RenderedCache_IsolatedByDocumentAuthorityAndRendererPolicy()
    {
        var cache = new DocumentCache();
        var document = new RenderedDocument(
            "<html />",
            [],
            new Dictionary<string, AnchorTarget>(),
            [],
            []);
        var key = new DocumentRenderCacheKey(
            "hash",
            "C:\\repo\\README.md",
            "C:\\repo",
            AppTheme.Light,
            EnableDiagrams: true,
            EnableMath: true,
            AllowRemoteAssets: false,
            RendererConfigurationVersion: "test-v1");

        cache.SetRendered(key, document);

        cache.TryGetRendered(key, out RenderedDocument? cached).Should().BeTrue();
        cached.Should().BeSameAs(document);
        cache.TryGetRendered(key with { SourcePath = "C:\\other\\README.md" }, out _).Should().BeFalse();
        cache.TryGetRendered(key with { AllowRemoteAssets = true }, out _).Should().BeFalse();
        cache.TryGetRendered(key with { RendererConfigurationVersion = "test-v2" }, out _).Should().BeFalse();
    }
}
