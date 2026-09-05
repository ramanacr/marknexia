using FluentAssertions;
using Marknexia.Core;
using Marknexia.Rendering;
using Xunit;

namespace Marknexia.Rendering.Tests;

public class MarkdownRendererTests
{
    private readonly MarkdownRenderer _renderer = new();

    [Fact]
    public async Task RenderAsync_FullDocument_ContainsThemeCssAndBridgeScript()
    {
        string markdown = "# Sample Title\n\nParagraph text.\n\n```csharp\nint x = 42;\n```";
        var context = new RenderContext("README.md", null, AppTheme.Dark);

        var doc = await _renderer.RenderAsync(markdown, context);

        doc.HtmlContent.Should().Contain("data-theme=\"dark\"");
        doc.HtmlContent.Should().Contain("<style>");
        doc.HtmlContent.Should().Contain("window.marknexiaBridge");
        doc.Headings.Should().HaveCount(1);
        doc.Headings[0].SlugId.Should().Be("sample-title");
        doc.AnchorIndex.Should().ContainKey("sample-title");
    }

    [Fact]
    public async Task RenderAsync_MermaidDiagram_InjectsDiagramContainer()
    {
        string markdown = "```mermaid\nsequenceDiagram\nAlice->>Bob: Hi\n```";
        var context = new RenderContext("diagram.md", null, AppTheme.Light);

        var doc = await _renderer.RenderAsync(markdown, context);

        doc.HtmlContent.Should().Contain("marknexia-diagram marknexia-mermaid");
        doc.HtmlContent.Should().Contain("marknexia-btn marknexia-btn-copy");
        doc.HtmlContent.Should().Contain("Copy");
        doc.HtmlContent.Should().Contain("sequenceDiagram");
    }
}
