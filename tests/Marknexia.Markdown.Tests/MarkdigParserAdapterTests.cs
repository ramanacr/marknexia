using FluentAssertions;
using Marknexia.Markdown;
using Xunit;

namespace Marknexia.Markdown.Tests;

public class MarkdigParserAdapterTests
{
    private readonly MarkdigParserAdapter _parser = new();

    [Fact]
    public void Parse_Headings_ExtractsHeadingsAndGeneratedSlugs()
    {
        string markdown = "# First Heading\n\n## Second Heading\n\n## Second Heading";
        var result = _parser.Parse(markdown);

        result.Headings.Should().HaveCount(3);
        result.Headings[0].SlugId.Should().Be("first-heading");
        result.Headings[1].SlugId.Should().Be("second-heading");
        result.Headings[2].SlugId.Should().Be("second-heading-1");
    }

    [Fact]
    public void Parse_MermaidFencedCodeBlock_ExtractsDiagramBlock()
    {
        string markdown = "```mermaid\ngraph TD\nA-->B\n```";
        var result = _parser.Parse(markdown);

        result.DiagramBlocks.Should().HaveCount(1);
        result.DiagramBlocks[0].DiagramType.Should().Be("mermaid");
        result.DiagramBlocks[0].SourceCode.Should().Contain("A-->B");
    }

    [Fact]
    public void Parse_GfmAlerts_TransformsToGitHubAlertMarkup()
    {
        string markdown = "> [!NOTE]\n> This is an important note.";
        var result = _parser.Parse(markdown);

        result.RenderedBodyHtml.Should().Contain("markdown-alert markdown-alert-note");
        result.RenderedBodyHtml.Should().Contain("This is an important note.");
    }

    [Fact]
    public void Parse_TaskLists_RendersTaskCheckboxes()
    {
        string markdown = "- [x] Done\n- [ ] Todo";
        var result = _parser.Parse(markdown);

        result.RenderedBodyHtml.Should().Contain("type=\"checkbox\"");
        result.RenderedBodyHtml.Should().Contain("checked");
    }

    [Fact]
    public void Parse_GfmTables_RendersHeaderAndCells()
    {
        string markdown = "| Name | Value |\n| --- | --- |\n| Marknexia | Native |";

        string html = _parser.Parse(markdown).RenderedBodyHtml;

        html.Should().Contain("<table>");
        html.Should().Contain("<th>Name</th>");
        html.Should().Contain("<td>Native</td>");
    }

    [Fact]
    public void Parse_Footnotes_RendersReferenceAndDefinition()
    {
        string markdown = "A reference[^1].\n\n[^1]: Supporting detail.";

        string html = _parser.Parse(markdown).RenderedBodyHtml;

        html.Should().Contain("footnote");
        html.Should().Contain("Supporting detail.");
    }

    [Fact]
    public void Parse_FencedCode_PreservesLanguageClass()
    {
        string html = _parser.Parse("```csharp\nvar answer = 42;\n```").RenderedBodyHtml;

        html.Should().Contain("language-csharp");
        html.Should().Contain("var answer = 42;");
    }

    [Fact]
    public void Parse_CustomAnchors_ExtractsAnchorsFromHtml()
    {
        string markdown = "<a name=\"my-anchor\"></a>\n\nContent";
        var result = _parser.Parse(markdown);

        result.CustomAnchors.Should().HaveCount(1);
        result.CustomAnchors[0].Name.Should().Be("my-anchor");
    }
}
