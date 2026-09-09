using FluentAssertions;
using Marknexia.Core;
using Marknexia.Rendering;
using Xunit;
using AngleSharp.Html.Parser;

namespace Marknexia.Rendering.Tests;

public class MarkdownRendererTests
{
    private readonly MarkdownRenderer _renderer = new();

    [Fact]
    public async Task RenderAsync_CodeCopy_PreservesExactPayloadWithoutExecutableAttributes()
    {
        const string code = "console.log(\"it's <safe> & Unicode: λ\");\n";
        var doc = await _renderer.RenderAsync($"```javascript\n{code}```", new RenderContext("README.md", null, AppTheme.Light));
        var html = new HtmlParser().ParseDocument(doc.HtmlContent);
        var button = html.QuerySelector(".code-container button");

        button.Should().NotBeNull("code blocks need an actual accessible copy button after sanitization");
        button!.GetAttribute("data-marknexia-action").Should().Be("copy");
        Uri.UnescapeDataString(button.GetAttribute("data-copy-text") ?? "").Should().Be(code);
        html.QuerySelectorAll(".markdown-body [onclick]").Should().BeEmpty();
    }

    [Fact]
    public async Task RenderAsync_MultipleDiagrams_PreserveIndependentSourcesAndControls()
    {
        const string markdown = "```mermaid\ngraph TD\n\nA-->B\n```\n\n```mermaid\nsequenceDiagram\nAlice->>Bob: $1 <hello>\n```";
        var doc = await _renderer.RenderAsync(markdown, new RenderContext("README.md", null, AppTheme.Light));
        var html = new HtmlParser().ParseDocument(doc.HtmlContent);
        var diagrams = html.QuerySelectorAll(".marknexia-mermaid");

        diagrams.Should().HaveCount(2);
        diagrams[0].QuerySelector(".mermaid")!.TextContent.TrimEnd().Should().Be("graph TD\n\nA-->B");
        diagrams[1].QuerySelector(".mermaid")!.TextContent.TrimEnd().Should().Be("sequenceDiagram\nAlice->>Bob: $1 <hello>");
        diagrams.Select(d => d.Id).Should().OnlyHaveUniqueItems();
        foreach (var diagram in diagrams)
        {
            var copy = diagram.QuerySelector("button[data-marknexia-action='copy']");
            copy.Should().NotBeNull();
            Uri.UnescapeDataString(copy!.GetAttribute("data-copy-text") ?? "").Should().Be(diagram.QuerySelector(".mermaid")!.TextContent);
            diagram.QuerySelector("button[data-marknexia-action='toggle-source']").Should().NotBeNull();
            diagram.QuerySelectorAll("[onclick]").Should().BeEmpty();
        }
    }

    [Fact]
    public async Task RenderAsync_FullDocument_ContainsThemeCssAndBridgeScript()
    {
        string markdown = "# Sample Title\n\nParagraph text.\n\n```csharp\nint x = 42;\n```";
        var context = new RenderContext("README.md", null, AppTheme.Dark);

        var doc = await _renderer.RenderAsync(markdown, context);

        doc.HtmlContent.Should().Contain("data-theme=\"dark\"");
        doc.HtmlContent.Should().Contain("<style>");
        doc.HtmlContent.Should().Contain("window.marknexiaBridge");
        doc.HtmlContent.Should().Contain("Segoe UI Variable");
        doc.HtmlContent.Should().NotContain("cdnjs.cloudflare.com");
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
        doc.HtmlContent.Should().Contain("https://marknexia.assets/mermaid.min.js");
        doc.HtmlContent.Should().NotContain("https://cdn.");
    }

    [Fact]
    public async Task RenderAsync_BlocksRemoteImagesByDefault()
    {
        var context = new RenderContext("README.md", null, AppTheme.Light);

        var doc = await _renderer.RenderAsync("![remote](https://example.com/image.png)", context);

        doc.HtmlContent.Should().Contain("<img");
        doc.HtmlContent.Should().NotContain("https://example.com/image.png");
    }

    [Fact]
    public async Task RenderAsync_Blocks_protocol_relative_and_srcset_remote_images_by_default()
    {
        var context = new RenderContext("README.md", null, AppTheme.Light);

        var doc = await _renderer.RenderAsync(
            "<img alt=\"remote\" src=\"//example.com/image.png\" srcset=\"//example.com/a.png 1x, https://example.com/b.png 2x\">",
            context);

        doc.HtmlContent.Should().NotContain("example.com");
    }

    [Fact]
    public async Task RenderAsync_AllowsRemoteImagesWhenExplicitlyEnabled()
    {
        var context = new RenderContext("README.md", null, AppTheme.Light, AllowRemoteAssets: true);

        var doc = await _renderer.RenderAsync("![remote](https://example.com/image.png)", context);

        doc.HtmlContent.Should().Contain("https://example.com/image.png");
    }

    [Fact]
    public async Task RenderAsync_ReturnsBeforeABusyParserCompletes()
    {
        var parser = new BlockingParserAdapter();
        var renderer = new MarkdownRenderer(parserAdapter: parser);
        var invocation = new TaskCompletionSource<Task<RenderedDocument>>(
            TaskCreationOptions.RunContinuationsAsynchronously);
        var thread = new Thread(() =>
        {
            try
            {
                invocation.SetResult(renderer.RenderAsync(
                    "# Async",
                    new RenderContext("README.md", null, AppTheme.Light)));
            }
            catch (Exception ex)
            {
                invocation.SetException(ex);
            }
        });
        thread.Start();

        parser.Started.Wait(TimeSpan.FromSeconds(2)).Should().BeTrue();
        try
        {
            Task completed = await Task.WhenAny(invocation.Task, Task.Delay(TimeSpan.FromMilliseconds(500)));
            completed.Should().Be(invocation.Task);
            Task<RenderedDocument> renderTask = await invocation.Task;
            parser.Release.Set();
            await renderTask;
        }
        finally
        {
            parser.Release.Set();
            thread.Join(TimeSpan.FromSeconds(2)).Should().BeTrue();
        }
    }

    [Fact]
    public async Task RenderAsync_RejectsRenderedHtmlAboveConfiguredLimit()
    {
        var parser = new FixedParserAdapter("<p>Rendered output</p>");
        var renderer = new MarkdownRenderer(
            parserAdapter: parser,
            renderedHtmlLimitBytes: 32);

        Func<Task> act = () => renderer.RenderAsync(
            "# Source",
            new RenderContext("README.md", null, AppTheme.Light));

        await act.Should().ThrowAsync<DocumentTooLargeException>()
            .WithMessage("*maximum supported size*");
    }

    [Fact]
    public async Task RenderAsync_DegradesExcessiveDiagramCountToAnAccessibleSourceFallback()
    {
        var renderer = new MarkdownRenderer(maxDiagramCount: 1);
        var doc = await renderer.RenderAsync(
            "```mermaid\ngraph TD\nA-->B\n```\n\n```mermaid\ngraph TD\nB-->C\n```",
            new RenderContext("README.md", null, AppTheme.Light));

        var html = new HtmlParser().ParseDocument(doc.HtmlContent);
        html.QuerySelectorAll(".marknexia-mermaid").Should().HaveCount(1);
        html.QuerySelector(".marknexia-diagram-fallback")!.TextContent
            .Should().Contain("diagram limit");
        html.QuerySelector(".marknexia-diagram-fallback details code")!.TextContent
            .Should().Contain("B-->C");
    }

    [Fact]
    public async Task RenderAsync_DegradesOversizedDiagramSourceWithoutLoadingMermaid()
    {
        var renderer = new MarkdownRenderer(maxDiagramSourceBytes: 8);
        var doc = await renderer.RenderAsync(
            "```mermaid\ngraph TD\nA-->B\n```",
            new RenderContext("README.md", null, AppTheme.Light));

        var html = new HtmlParser().ParseDocument(doc.HtmlContent);
        html.QuerySelector(".marknexia-mermaid").Should().BeNull();
        html.QuerySelector(".marknexia-diagram-fallback")!.TextContent
            .Should().Contain("source limit");
        doc.HtmlContent.Should().NotContain("mermaid.min.js");
    }

    [Fact]
    public async Task RenderAsync_DisablesDiagramExecutionWhenTheContextDisablesDiagrams()
    {
        var doc = await _renderer.RenderAsync(
            "```mermaid\ngraph TD\nA-->B\n```",
            new RenderContext("README.md", null, AppTheme.Light, EnableDiagrams: false));

        var html = new HtmlParser().ParseDocument(doc.HtmlContent);
        html.QuerySelector(".marknexia-diagram-fallback")!.TextContent
            .Should().Contain("diagram rendering is disabled");
        html.QuerySelector(".marknexia-mermaid").Should().BeNull();
        html.QuerySelector("script[src*='mermaid.min.js']").Should().BeNull();
    }

    [Fact]
    public async Task RenderAsync_Emits_an_origin_scoped_content_security_policy_and_nonces_trusted_scripts()
    {
        string sourcePath = Path.Combine(Path.GetTempPath(), "marknexia-csp", "README.md");
        var doc = await _renderer.RenderAsync(
            "# Safe\n\n![local](images/logo.png)\n\n```mermaid\ngraph TD\nA-->B\n```",
            new RenderContext(sourcePath, null, AppTheme.Light));
        var html = new HtmlParser().ParseDocument(doc.HtmlContent);
        string policy = html.QuerySelector("meta[http-equiv='Content-Security-Policy']")?.GetAttribute("content") ?? "";

        policy.Should().Contain("default-src 'none'");
        policy.Should().Contain($"img-src {doc.AssetContext!.Origin} data:");
        policy.Should().Contain("base-uri ");
        policy.Should().Contain("form-action 'none'");
        policy.Should().NotContain("img-src *");

        string nonce = html.QuerySelector("script")?.GetAttribute("nonce") ?? "";
        nonce.Should().NotBeNullOrWhiteSpace();
        policy.Should().Contain($"'nonce-{nonce}'");
        html.QuerySelectorAll("script").Should().OnlyContain(script => script.GetAttribute("nonce") == nonce);
    }

    [Fact]
    public async Task RenderAsync_Renders_inline_and_display_math_as_local_accessible_markup()
    {
        var doc = await _renderer.RenderAsync(
            "Inline $x^2 + \\alpha$\n\n$$\n\\frac{a}{b}\n$$",
            new RenderContext("README.md", null, AppTheme.Light));

        doc.HtmlContent.Should().Contain("marknexia-math");
        doc.HtmlContent.Should().Contain("role=\"math\"");
        doc.HtmlContent.Should().Contain("<sup>2</sup>");
        doc.HtmlContent.Should().Contain("marknexia-math-display");
        doc.HtmlContent.Should().NotContain("katex");
    }

    [Fact]
    public void EnsureAssetsExtracted_repairs_a_same_length_tampered_bundle()
    {
        string directory = Path.Combine(Path.GetTempPath(), $"marknexia-assets-{Guid.NewGuid():N}");
        try
        {
            TemplateEngine.EnsureAssetsExtracted(directory);
            string path = Path.Combine(directory, "mermaid.min.js");
            byte[] expected = File.ReadAllBytes(path);
            expected[expected.Length / 2] ^= 0x01;
            File.WriteAllBytes(path, expected);

            TemplateEngine.EnsureAssetsExtracted(directory);

            File.ReadAllBytes(path).Should().NotEqual(expected);
        }
        finally
        {
            if (Directory.Exists(directory)) Directory.Delete(directory, recursive: true);
        }
    }

    private sealed class BlockingParserAdapter : IMarkdownParserAdapter
    {
        public ManualResetEventSlim Started { get; } = new();
        public ManualResetEventSlim Release { get; } = new();

        public ParsedMarkdown Parse(string markdownSource)
        {
            Started.Set();
            Release.Wait(TimeSpan.FromSeconds(5));
            return new ParsedMarkdown(
                "<h1 id=\"async\">Async</h1>",
                [new HeadingInfo("Async", 1, "async")],
                [],
                [],
                [],
                [],
                []);
        }
    }

    private sealed class FixedParserAdapter : IMarkdownParserAdapter
    {
        private readonly string _html;

        public FixedParserAdapter(string html) => _html = html;

        public ParsedMarkdown Parse(string markdownSource) => new(
            _html,
            [],
            [],
            [],
            [],
            [],
            []);
    }
}
