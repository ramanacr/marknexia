using System.Text;
using System.Text.RegularExpressions;
using Marknexia.Core;
using Marknexia.Diagrams;
using Marknexia.Markdown;
using Marknexia.Security;
using Marknexia.Syntax;

namespace Marknexia.Rendering;

public sealed class MarkdownRenderer : IMarkdownRenderer
{
    public const long MaxRenderedHtmlBytes = 128L * 1024 * 1024;
    public const int MaxDiagramCount = 64;
    public const long MaxDiagramSourceBytes = 1L * 1024 * 1024;

    private readonly IMarkdownParserAdapter _parserAdapter;
    private readonly IHtmlSanitizer _sanitizer;
    private readonly ISyntaxHighlighter _syntaxHighlighter;
    private readonly DiagramRegistry _diagramRegistry;
    private readonly TemplateEngine _templateEngine;
    private readonly IMathRenderer _mathRenderer;
    private readonly long _maxRenderedHtmlBytes;
    private readonly int _maxDiagramCount;
    private readonly long _maxDiagramSourceBytes;
    private readonly object _sanitizerLock = new();
    private readonly object _syntaxHighlighterLock = new();

    public MarkdownRenderer(
        IMarkdownParserAdapter? parserAdapter = null,
        IHtmlSanitizer? sanitizer = null,
        ISyntaxHighlighter? syntaxHighlighter = null,
        DiagramRegistry? diagramRegistry = null,
        TemplateEngine? templateEngine = null,
        IMathRenderer? mathRenderer = null,
        long renderedHtmlLimitBytes = MaxRenderedHtmlBytes,
        int maxDiagramCount = MaxDiagramCount,
        long maxDiagramSourceBytes = MaxDiagramSourceBytes)
    {
        if (renderedHtmlLimitBytes <= 0) throw new ArgumentOutOfRangeException(nameof(renderedHtmlLimitBytes));
        if (maxDiagramCount < 0) throw new ArgumentOutOfRangeException(nameof(maxDiagramCount));
        if (maxDiagramSourceBytes <= 0) throw new ArgumentOutOfRangeException(nameof(maxDiagramSourceBytes));

        _parserAdapter = parserAdapter ?? new MarkdigParserAdapter();
        _sanitizer = sanitizer ?? new HtmlSanitizerService();
        _syntaxHighlighter = syntaxHighlighter ?? new ColorCodeSyntaxHighlighter();
        _diagramRegistry = diagramRegistry ?? new DiagramRegistry();
        _templateEngine = templateEngine ?? new TemplateEngine();
        _mathRenderer = mathRenderer ?? new SimpleMathRenderer();
        _maxRenderedHtmlBytes = renderedHtmlLimitBytes;
        _maxDiagramCount = maxDiagramCount;
        _maxDiagramSourceBytes = maxDiagramSourceBytes;
    }

    public Task<RenderedDocument> RenderAsync(string markdownSource, RenderContext context, CancellationToken cancellationToken = default)
    {
        cancellationToken.ThrowIfCancellationRequested();

        return Task.Run(() => RenderCore(markdownSource, context, cancellationToken), cancellationToken);
    }

    private RenderedDocument RenderCore(string markdownSource, RenderContext context, CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();

        // 1. Parse Markdown to AST & Initial HTML
        ParsedMarkdown parsed = _parserAdapter.Parse(markdownSource);
        cancellationToken.ThrowIfCancellationRequested();

        string bodyHtml = parsed.RenderedBodyHtml;

        // 2. Highlight ordinary code before adding diagram source controls.
        bodyHtml = HighlightCodeBlocks(bodyHtml, cancellationToken);

        // 3. Transform each Mermaid block independently. Use the rendered code
        // text so blank lines survive and replacement tokens such as $1 remain
        // literal, instead of substituting one diagram into every code block.
        int diagramCounter = 0;
        bool hasMermaid = false;
        bodyHtml = Regex.Replace(bodyHtml, @"<pre><code\s+class=""language-mermaid"">([\s\S]*?)</code></pre>", match =>
        {
            cancellationToken.ThrowIfCancellationRequested();
            string source = System.Net.WebUtility.HtmlDecode(match.Groups[1].Value);
            int diagramNumber = ++diagramCounter;
            if (!context.EnableDiagrams)
            {
                return RenderDiagramFallback(source, "diagram rendering is disabled for this document");
            }

            if (diagramNumber > _maxDiagramCount)
            {
                return RenderDiagramFallback(source, "diagram limit exceeded");
            }

            if (Encoding.UTF8.GetByteCount(source) > _maxDiagramSourceBytes)
            {
                return RenderDiagramFallback(source, "diagram source limit exceeded");
            }

            hasMermaid = true;
            return _diagramRegistry.RenderDiagram("mermaid", source, $"mermaid-{diagramNumber}");
        }, RegexOptions.IgnoreCase);

        // Markdig's Mathematics extension intentionally emits delimiters. Convert
        // them before sanitization so the final document has no math CDN/runtime
        // dependency and all user input remains encoded by the local renderer.
        bodyHtml = RenderMath(bodyHtml, context.EnableMath, cancellationToken);

        // 4. Sanitize HTML
        string sanitizedHtml;
        lock (_sanitizerLock)
        {
            sanitizedHtml = _sanitizer.SanitizeHtml(bodyHtml);
        }
        if (!context.AllowRemoteAssets)
        {
            sanitizedHtml = BlockRemoteImages(sanitizedHtml);
        }

        // 5. Wrap in Full HTML Document with CSS and Bridge JS
        var assetContext = DocumentAssetContext.Create(context.SourcePath, context.RepositoryRoot);
        string fullDocumentHtml = _templateEngine.GenerateHtml(sanitizedHtml, context, hasMermaid, assetContext);
        long renderedHtmlBytes = Encoding.UTF8.GetByteCount(fullDocumentHtml);
        if (renderedHtmlBytes > _maxRenderedHtmlBytes)
        {
            throw new DocumentTooLargeException(context.SourcePath, renderedHtmlBytes, _maxRenderedHtmlBytes);
        }

        // 6. Assemble Anchor Index
        var anchorIndex = new Dictionary<string, AnchorTarget>(StringComparer.OrdinalIgnoreCase);

        foreach (HeadingInfo h in parsed.Headings)
        {
            if (!anchorIndex.ContainsKey(h.SlugId))
            {
                anchorIndex[h.SlugId] = new AnchorTarget(h.SlugId, h.Text, true, h.LineNumber);
            }
        }

        foreach (AnchorTarget custom in parsed.CustomAnchors)
        {
            if (!anchorIndex.ContainsKey(custom.Id))
            {
                anchorIndex[custom.Id] = custom;
            }
        }

        var result = new RenderedDocument(
            fullDocumentHtml,
            parsed.Headings,
            anchorIndex,
            parsed.ExtractedImages,
            parsed.Diagnostics,
            assetContext);

        return result;
    }

    private static string BlockRemoteImages(string html)
    {
        string withoutRemoteSrc = Regex.Replace(
            html,
            @"<img\b(?<attributes>[^>]*?)\bsrc\s*=\s*(?<quote>[""'])(?:https?://|//)[^""']*\k<quote>(?<tail>[^>]*)>",
            match => $"<img{match.Groups["attributes"].Value}src=\"\"{match.Groups["tail"].Value}>",
            RegexOptions.IgnoreCase);

        return Regex.Replace(
            withoutRemoteSrc,
            @"\s+srcset\s*=\s*(?<quote>[""'])(?=[^""']*(?:https?://|//))[^""']*\k<quote>",
            string.Empty,
            RegexOptions.IgnoreCase);
    }

    private string RenderMath(string html, bool enabled, CancellationToken cancellationToken)
    {
        const string mathPattern = @"<(?<tag>span|div)\s+class=""math"">(?<expression>\s*(?:\\\(|\\\[|\$\$)[\s\S]*?(?:\\\)|\\\]|\$\$)\s*)</\k<tag>>";

        return Regex.Replace(html, mathPattern, match =>
        {
            cancellationToken.ThrowIfCancellationRequested();
            string expression = match.Groups["expression"].Value;
            string trimmedExpression = expression.Trim();
            bool display = trimmedExpression.StartsWith(@"\[", StringComparison.Ordinal)
                || trimmedExpression.StartsWith("$$", StringComparison.Ordinal);
            return enabled
                ? _mathRenderer.Render(expression, display ? MathMode.Display : MathMode.Inline).HtmlContent
                : $"<{(display ? "div" : "span")} class=\"marknexia-math-fallback\" role=\"math\">{System.Net.WebUtility.HtmlEncode(expression)}</{(display ? "div" : "span")}>";
        }, RegexOptions.IgnoreCase);
    }

    private string HighlightCodeBlocks(string html, CancellationToken cancellationToken)
    {
        string pattern = @"<pre><code(?:\s+class=""language-([a-zA-Z0-9_\+#\-]+)"")?>([\s\S]*?)</code></pre>";
        return Regex.Replace(html, pattern, match =>
        {
            cancellationToken.ThrowIfCancellationRequested();
            string lang = match.Groups[1].Value;
            if (lang.Equals("mermaid", StringComparison.OrdinalIgnoreCase)) return match.Value;
            string rawCode = match.Groups[2].Value;
            string decodedCode = System.Net.WebUtility.HtmlDecode(rawCode);

            string highlighted;
            lock (_syntaxHighlighterLock)
            {
                highlighted = _syntaxHighlighter.HighlightCode(decodedCode, lang);
            }

            string uriEscaped = Uri.EscapeDataString(decodedCode);
            return $@"
<div class=""code-container"">
  <button type=""button"" class=""copy-btn"" data-marknexia-action=""copy"" data-copy-text=""{uriEscaped}"" title=""Copy code"">Copy</button>
  {highlighted}
</div>";
        });
    }

    private static string RenderDiagramFallback(string source, string reason)
    {
        string encodedSource = System.Net.WebUtility.HtmlEncode(source);
        string encodedReason = System.Net.WebUtility.HtmlEncode(reason);
        return $@"
<div class=""marknexia-diagram-fallback"" role=""note""><strong>Mermaid diagram was not rendered.</strong>
  <p>{encodedReason}.</p>
  <details><summary>Show diagram source</summary><pre><code>{encodedSource}</code></pre></details>
</div>";
    }
}
