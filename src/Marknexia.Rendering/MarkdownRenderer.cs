using System.Text.RegularExpressions;
using Marknexia.Core;
using Marknexia.Diagrams;
using Marknexia.Markdown;
using Marknexia.Security;
using Marknexia.Syntax;

namespace Marknexia.Rendering;

public sealed class MarkdownRenderer : IMarkdownRenderer
{
    private readonly IMarkdownParserAdapter _parserAdapter;
    private readonly IHtmlSanitizer _sanitizer;
    private readonly ISyntaxHighlighter _syntaxHighlighter;
    private readonly DiagramRegistry _diagramRegistry;
    private readonly TemplateEngine _templateEngine;

    public MarkdownRenderer(
        IMarkdownParserAdapter? parserAdapter = null,
        IHtmlSanitizer? sanitizer = null,
        ISyntaxHighlighter? syntaxHighlighter = null,
        DiagramRegistry? diagramRegistry = null,
        TemplateEngine? templateEngine = null)
    {
        _parserAdapter = parserAdapter ?? new MarkdigParserAdapter();
        _sanitizer = sanitizer ?? new HtmlSanitizerService();
        _syntaxHighlighter = syntaxHighlighter ?? new ColorCodeSyntaxHighlighter();
        _diagramRegistry = diagramRegistry ?? new DiagramRegistry();
        _templateEngine = templateEngine ?? new TemplateEngine();
    }

    public Task<RenderedDocument> RenderAsync(string markdownSource, RenderContext context, CancellationToken cancellationToken = default)
    {
        cancellationToken.ThrowIfCancellationRequested();

        // 1. Parse Markdown to AST & Initial HTML
        ParsedMarkdown parsed = _parserAdapter.Parse(markdownSource);

        string bodyHtml = parsed.RenderedBodyHtml;

        // 2. Replace Mermaid Diagram blocks
        bool hasMermaid = false;
        foreach (DiagramBlock diagram in parsed.DiagramBlocks)
        {
            cancellationToken.ThrowIfCancellationRequested();

            if (diagram.DiagramType.Equals("mermaid", StringComparison.OrdinalIgnoreCase))
            {
                hasMermaid = true;
                string diagramHtml = _diagramRegistry.RenderDiagram(diagram.DiagramType, diagram.SourceCode, diagram.Id);

                // Replace the rendered <pre><code class="language-mermaid">...</code></pre> with diagram HTML
                string codeBlockPattern = @"<pre><code\s+class=""language-mermaid"">" + Regex.Escape(System.Net.WebUtility.HtmlEncode(diagram.SourceCode)) + @"</code></pre>";
                if (Regex.IsMatch(bodyHtml, codeBlockPattern))
                {
                    bodyHtml = Regex.Replace(bodyHtml, codeBlockPattern, diagramHtml, RegexOptions.None);
                }
                else
                {
                    // If exact match fails due to minor whitespace, replace first matching mermaid block
                    bodyHtml = Regex.Replace(bodyHtml, @"<pre><code\s+class=""language-mermaid"">[\s\S]*?</code></pre>", diagramHtml, RegexOptions.None);
                }
            }
        }

        // 3. Syntax highlight remaining code blocks
        bodyHtml = HighlightCodeBlocks(bodyHtml);

        // 4. Sanitize HTML
        string sanitizedHtml = _sanitizer.SanitizeHtml(bodyHtml);

        // 5. Wrap in Full HTML Document with CSS and Bridge JS
        string fullDocumentHtml = _templateEngine.GenerateHtml(sanitizedHtml, context, hasMermaid);

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
            parsed.Diagnostics);

        return Task.FromResult(result);
    }

    private string HighlightCodeBlocks(string html)
    {
        string pattern = @"<pre><code(?:\s+class=""language-([a-zA-Z0-9_\+#\-]+)"")?>([\s\S]*?)</code></pre>";
        return Regex.Replace(html, pattern, match =>
        {
            string lang = match.Groups[1].Value;
            string rawCode = match.Groups[2].Value;
            string decodedCode = System.Net.WebUtility.HtmlDecode(rawCode);

            string highlighted = _syntaxHighlighter.HighlightCode(decodedCode, lang);

            string uriEscaped = Uri.EscapeDataString(decodedCode);
            return $@"
<div class=""code-container"">
  <button type=""button"" class=""copy-btn"" onclick=""window.marknexiaBridge.copyText(decodeURIComponent('{uriEscaped}'))"">Copy</button>
  {highlighted}
</div>";
        });
    }
}
