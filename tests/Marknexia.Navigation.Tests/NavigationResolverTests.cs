using FluentAssertions;
using Marknexia.Core;
using Marknexia.Files;
using Marknexia.Navigation;
using Xunit;

namespace Marknexia.Navigation.Tests;

public class NavigationResolverTests
{
    private readonly NavigationResolver _resolver;
    private readonly PathCanonicalizer _canonicalizer;
    private readonly FileService _fileService;
    private readonly string _repoRoot;

    public NavigationResolverTests()
    {
        _canonicalizer = new PathCanonicalizer();
        _fileService = new FileService(_canonicalizer);
        _resolver = new NavigationResolver(_canonicalizer, _fileService);
        _repoRoot = FindRepoRoot();
    }

    private static string FindRepoRoot()
    {
        string? dir = AppContext.BaseDirectory;
        while (!string.IsNullOrEmpty(dir))
        {
            string candidate = Path.Combine(dir, "test-fixtures", "markdown", "navigation");
            if (Directory.Exists(candidate))
            {
                return Path.GetFullPath(candidate);
            }
            DirectoryInfo? parent = Directory.GetParent(dir);
            if (parent == null || parent.FullName.Equals(dir, StringComparison.OrdinalIgnoreCase))
            {
                break;
            }
            dir = parent.FullName;
        }
        return Path.GetFullPath(@"test-fixtures\markdown\navigation");
    }

    [Theory]
    [InlineData("", UriClassification.Empty)]
    [InlineData("   ", UriClassification.Empty)]
    [InlineData("#overview", UriClassification.FragmentOnly)]
    [InlineData("./docs/api.md", UriClassification.RelativePath)]
    [InlineData("../README.md", UriClassification.RelativePath)]
    [InlineData("docs/api.md#authentication", UriClassification.RelativePath)]
    [InlineData("/docs/api.md", UriClassification.RepositoryRootPath)]
    [InlineData("https://github.com", UriClassification.Https)]
    [InlineData("http://example.com", UriClassification.Http)]
    [InlineData("mailto:test@example.com", UriClassification.OtherExternal)]
    [InlineData("javascript:alert(1)", UriClassification.Unsupported)]
    [InlineData("vbscript:msgbox(1)", UriClassification.Unsupported)]
    public void Classify_ReturnsExpectedClassification(string destination, UriClassification expected)
    {
        var result = _resolver.Classify(destination);
        result.Should().Be(expected);
    }

    [Fact]
    public void Resolve_FragmentOnly_ReturnsSameDocumentAnchor()
    {
        string currentFile = Path.Combine(_repoRoot, "README.md");
        var context = new ResolutionContext(currentFile, _repoRoot, null, new NavigationPolicy());

        var intent = _resolver.Resolve("#features-section", context);

        intent.Kind.Should().Be(NavigationKind.SameDocumentAnchor);
        intent.Fragment.Should().Be("features-section");
        intent.IsSafe.Should().BeTrue();
    }

    [Fact]
    public void Resolve_RelativeFile_ResolvesExistingDocument()
    {
        string currentFile = Path.Combine(_repoRoot, "README.md");
        var context = new ResolutionContext(currentFile, _repoRoot, null, new NavigationPolicy());

        var intent = _resolver.Resolve("docs/api.md", context);

        intent.Kind.Should().Be(NavigationKind.CrossDocument);
        intent.TargetDocument.Should().NotBeNull();
        intent.TargetDocument!.CanonicalPath.Should().EndWith(@"docs\api.md");
        intent.IsSafe.Should().BeTrue();
    }

    [Fact]
    public void Resolve_RelativeFileWithFragment_ResolvesCrossDocumentWithAnchor()
    {
        string currentFile = Path.Combine(_repoRoot, "README.md");
        var context = new ResolutionContext(currentFile, _repoRoot, null, new NavigationPolicy());

        var intent = _resolver.Resolve("docs/api.md#authentication", context);

        intent.Kind.Should().Be(NavigationKind.CrossDocumentWithAnchor);
        intent.Fragment.Should().Be("authentication");
        intent.TargetDocument.Should().NotBeNull();
        intent.TargetDocument!.CanonicalPath.Should().EndWith(@"docs\api.md");
    }

    [Fact]
    public void Resolve_RepositoryRootLink_WithRepoContext_ResolvesCorrectly()
    {
        string currentFile = Path.Combine(_repoRoot, "docs", "nested", "details.md");
        var context = new ResolutionContext(currentFile, _repoRoot, null, new NavigationPolicy());

        var intent = _resolver.Resolve("/docs/api.md", context);

        intent.Kind.Should().Be(NavigationKind.CrossDocument);
        intent.TargetDocument.Should().NotBeNull();
        intent.TargetDocument!.CanonicalPath.Should().EndWith(@"docs\api.md");
    }

    [Fact]
    public void Resolve_RepositoryRootLink_WithoutRepoContext_ReturnsBrokenTarget()
    {
        string currentFile = Path.Combine(_repoRoot, "README.md");
        var context = new ResolutionContext(currentFile, null, null, new NavigationPolicy());

        var intent = _resolver.Resolve("/docs/api.md", context);

        intent.Kind.Should().Be(NavigationKind.BrokenTarget);
        intent.Diagnostic.Should().Contain("No active repository context");
    }

    [Fact]
    public void Resolve_SandboxTraversal_EscapingRoot_IsBlocked()
    {
        string currentFile = Path.Combine(_repoRoot, "README.md");
        var context = new ResolutionContext(currentFile, _repoRoot, null, new NavigationPolicy(EnforceRepositorySandbox: true));

        var intent = _resolver.Resolve("../../../../Windows/System32/calc.exe", context);

        intent.Kind.Should().Be(NavigationKind.BlockedOrInvalid);
        intent.IsSafe.Should().BeFalse();
    }

    [Fact]
    public void Resolve_ExternalHttps_ReturnsExternalBrowserIntent()
    {
        string currentFile = Path.Combine(_repoRoot, "README.md");
        var context = new ResolutionContext(currentFile, _repoRoot, null, new NavigationPolicy());

        var intent = _resolver.Resolve("https://github.com", context);

        intent.Kind.Should().Be(NavigationKind.ExternalBrowser);
        intent.ExternalUri.Should().Be(new Uri("https://github.com"));
        intent.IsSafe.Should().BeTrue();
    }

    [Fact]
    public void Resolve_JavascriptScheme_IsBlocked()
    {
        string currentFile = Path.Combine(_repoRoot, "README.md");
        var context = new ResolutionContext(currentFile, _repoRoot, null, new NavigationPolicy());

        var intent = _resolver.Resolve("javascript:alert(1)", context);

        intent.Kind.Should().Be(NavigationKind.BlockedOrInvalid);
        intent.IsSafe.Should().BeFalse();
    }
}
