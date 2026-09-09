using AngleSharp.Html.Parser;
using FluentAssertions;
using Marknexia.Core;
using Marknexia.Rendering;
using Xunit;

namespace Marknexia.Rendering.Tests;

public sealed class DocumentAssetBaseTests
{
    [Theory]
    [InlineData("docs/nested", "/docs/nested/")]
    [InlineData("docs/space name", "/docs/space%20name/")]
    [InlineData("docs/literal%20", "/docs/literal%2520/")]
    public async Task Render_NestedDocument_UsesItsDirectoryWithinTheAssetRoot(string directory, string expectedPath)
    {
        string root = Path.Combine(Path.GetTempPath(), "marknexia-asset-base");
        string source = Path.Combine(root, directory, "README.md");
        var result = await new MarkdownRenderer().RenderAsync("![local](image.png)", new RenderContext(source, root, AppTheme.Light));
        var html = new HtmlParser().ParseDocument(result.HtmlContent);
        var basis = new Uri(html.QuerySelector("base")!.GetAttribute("href")!);

        basis.AbsolutePath.Should().Be(expectedPath);
        new Uri(basis, "image.png").AbsolutePath.Should().Be(expectedPath + "image.png");
        new Uri(basis, "/images/root.png").AbsolutePath.Should().Be("/images/root.png");
    }

    [Fact]
    public async Task Render_OutsideExplicitRoot_DoesNotSilentlyBroadenAssetAccess()
    {
        string root = Path.Combine(Path.GetTempPath(), "marknexia-asset-base");
        string source = Path.Combine(Path.GetTempPath(), "unrelated", "README.md");
        Func<Task> render = () => new MarkdownRenderer().RenderAsync("![image](image.png)", new RenderContext(source, root, AppTheme.Light));

        await render.Should().ThrowAsync<ArgumentException>();
    }

    [Fact]
    public void StandaloneDocument_UsesParentDirectoryAsBoundedAssetRoot()
    {
        string root = Path.Combine(Path.GetTempPath(), "marknexia-asset-base");
        string source = Path.Combine(root, "docs", "README.md");

        var context = DocumentAssetContext.Create(source, repositoryRoot: null);

        context.RootPath.Should().Be(root);
        new Uri(context.BaseUri, "../images/logo.png").AbsolutePath.Should().Be("/images/logo.png");
    }
}
