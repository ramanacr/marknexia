using System.Text;
using System.Text.RegularExpressions;
using Markdig;
using Markdig.Extensions.AutoIdentifiers;
using Markdig.Renderers.Html;
using Markdig.Syntax;
using Markdig.Syntax.Inlines;
using Marknexia.Core;

namespace Marknexia.Markdown;

public sealed class MarkdigParserAdapter : IMarkdownParserAdapter
{
    private readonly MarkdownPipeline _pipeline;
    public MarkdigParserAdapter()
    {
        _pipeline = new MarkdownPipelineBuilder()
            .UseAutoIdentifiers(AutoIdentifierOptions.GitHub)
            .UsePipeTables()
            .UseGridTables()
            .UseTaskLists()
            .UseFootnotes()
            .UseEmphasisExtras()
            .UseAutoLinks()
            .UseMathematics()
            .Build();
    }

    public ParsedMarkdown Parse(string markdownSource)
    {
        if (string.IsNullOrEmpty(markdownSource))
        {
            return new ParsedMarkdown(
                string.Empty,
                Array.Empty<HeadingInfo>(),
                Array.Empty<AnchorTarget>(),
                Array.Empty<string>(),
                Array.Empty<string>(),
                Array.Empty<DiagramBlock>(),
                Array.Empty<DiagnosticInfo>());
        }

        var slugGenerator = new HeadingSlugGenerator();

        MarkdownDocument document = Markdig.Markdown.Parse(markdownSource, _pipeline);

        var headings = new List<HeadingInfo>();
        var customAnchors = new List<AnchorTarget>();
        var links = new List<string>();
        var images = new List<string>();
        var diagrams = new List<DiagramBlock>();
        var diagnostics = new List<DiagnosticInfo>();

        int diagramCounter = 0;

        foreach (Block block in document)
        {
            InspectBlock(block, headings, customAnchors, links, images, diagrams, slugGenerator, ref diagramCounter);
        }

        // Render AST to HTML
        string rawHtml = Markdig.Markdown.ToHtml(document, _pipeline);

        // Transform GFM Alert blockquotes: > [!NOTE], > [!TIP], > [!IMPORTANT], > [!WARNING], > [!CAUTION]
        string transformedHtml = TransformGfmAlerts(rawHtml);

        return new ParsedMarkdown(
            transformedHtml,
            headings,
            customAnchors,
            links,
            images,
            diagrams,
            diagnostics);
    }

    private void InspectBlock(
        Block block,
        List<HeadingInfo> headings,
        List<AnchorTarget> customAnchors,
        List<string> links,
        List<string> images,
        List<DiagramBlock> diagrams,
        HeadingSlugGenerator slugGenerator,
        ref int diagramCounter)
    {
        if (block is HeadingBlock heading)
        {
            string headingText = ExtractPlainText(heading.Inline);
            string slugId = slugGenerator.GenerateSlug(headingText);
            heading.GetAttributes().Id = slugId;
            headings.Add(new HeadingInfo(headingText, heading.Level, slugId, heading.Line));
        }
        else if (block is FencedCodeBlock codeBlock)
        {
            string info = codeBlock.Info?.Trim() ?? string.Empty;
            if (info.Equals("mermaid", StringComparison.OrdinalIgnoreCase))
            {
                diagramCounter++;
                string diagramId = $"mermaid-{diagramCounter}";
                string source = GetCodeBlockText(codeBlock);
                diagrams.Add(new DiagramBlock(diagramId, "mermaid", source, codeBlock.Line));
            }
        }
        else if (block is HtmlBlock htmlBlock)
        {
            ExtractAnchorsFromHtml(htmlBlock.Lines.ToString(), customAnchors, htmlBlock.Line);
        }

        if (block is LeafBlock leaf && leaf.Inline != null)
        {
            InspectInlines(leaf.Inline, customAnchors, links, images, leaf.Line);
        }
        else if (block is ContainerBlock container)
        {
            foreach (Block child in container)
            {
                InspectBlock(child, headings, customAnchors, links, images, diagrams, slugGenerator, ref diagramCounter);
            }
        }
    }

    private void InspectInlines(
        ContainerInline parentInline,
        List<AnchorTarget> customAnchors,
        List<string> links,
        List<string> images,
        int lineNumber)
    {
        foreach (Inline inline in parentInline)
        {
            if (inline is LinkInline link)
            {
                if (!string.IsNullOrEmpty(link.Url))
                {
                    if (link.IsImage)
                    {
                        images.Add(link.Url);
                    }
                    else
                    {
                        links.Add(link.Url);
                    }
                }
            }
            else if (inline is HtmlInline htmlInline)
            {
                ExtractAnchorsFromHtml(htmlInline.Tag, customAnchors, lineNumber);
            }

            if (inline is ContainerInline childContainer)
            {
                InspectInlines(childContainer, customAnchors, links, images, lineNumber);
            }
        }
    }

    private static string ExtractPlainText(ContainerInline? inline)
    {
        if (inline == null) return string.Empty;
        var sb = new StringBuilder();
        foreach (Inline item in inline)
        {
            if (item is LiteralInline lit)
            {
                sb.Append(lit.Content);
            }
            else if (item is CodeInline code)
            {
                sb.Append(code.Content);
            }
            else if (item is ContainerInline container)
            {
                sb.Append(ExtractPlainText(container));
            }
        }
        return sb.ToString().Trim();
    }

    private static string GetCodeBlockText(FencedCodeBlock codeBlock)
    {
        var sb = new StringBuilder();
        foreach (var line in codeBlock.Lines.Lines)
        {
            string slice = line.Slice.ToString();
            if (!string.IsNullOrEmpty(slice))
            {
                sb.AppendLine(slice);
            }
        }
        return sb.ToString().TrimEnd();
    }

    private static void ExtractAnchorsFromHtml(string html, List<AnchorTarget> customAnchors, int line)
    {
        if (string.IsNullOrWhiteSpace(html)) return;

        var matches = Regex.Matches(html, @"<a\s+[^>]*(?:name|id)\s*=\s*[""']([^""']+)[""'][^>]*>", RegexOptions.IgnoreCase);
        foreach (Match match in matches)
        {
            if (match.Groups.Count > 1)
            {
                string anchorName = match.Groups[1].Value;
                customAnchors.Add(new AnchorTarget(anchorName, anchorName, false, line));
            }
        }
    }

    private static string TransformGfmAlerts(string html)
    {
        if (string.IsNullOrWhiteSpace(html)) return html;

        string pattern = @"<blockquote>\s*<p>\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\](?:\s*<br\s*/?>|\s*\r?\n)?(.*?)</p>\s*(.*?)</blockquote>";
        return Regex.Replace(html, pattern, match =>
        {
            string alertType = match.Groups[1].Value.ToLowerInvariant();
            string title = match.Groups[1].Value;
            string firstLine = match.Groups[2].Value.Trim();
            string remainder = match.Groups[3].Value.Trim();

            string iconSvg = GetAlertIconSvg(alertType);

            var sb = new StringBuilder();
            sb.Append($@"<div class=""markdown-alert markdown-alert-{alertType}"">");
            sb.Append($@"<div class=""markdown-alert-title"">{iconSvg}<span>{title}</span></div>");
            if (!string.IsNullOrEmpty(firstLine))
            {
                sb.Append($"<p>{firstLine}</p>");
            }
            if (!string.IsNullOrEmpty(remainder))
            {
                sb.Append(remainder);
            }
            sb.Append("</div>");
            return sb.ToString();
        }, RegexOptions.Singleline | RegexOptions.IgnoreCase);
    }

    private static string GetAlertIconSvg(string type)
    {
        return type switch
        {
            "tip" => @"<svg class=""octicon octicon-light-bulb"" viewBox=""0 0 16 16"" width=""16"" height=""16"" aria-hidden=""true""><path d=""M8 1.5c-2.363 0-4 1.69-4 3.75 0 .76.24 1.487.653 2.083.47.677.847 1.549.847 2.417h5c0-.868.377-1.74.847-2.417.413-.596.653-1.323.653-2.083 0-2.06-1.637-3.75-4-3.75Z""></path></svg>",
            "important" => @"<svg class=""octicon octicon-report"" viewBox=""0 0 16 16"" width=""16"" height=""16"" aria-hidden=""true""><path d=""M0 1.75C0 .784.784 0 1.75 0h12.5C15.216 0 16 .784 16 1.75v9.5A1.75 1.75 0 0 1 14.25 13H9.06l-2.573 2.573A1.458 1.458 0 0 1 4 14.543V13H1.75A1.75 1.75 0 0 1 0 11.25Z""></path></svg>",
            "warning" => @"<svg class=""octicon octicon-alert"" viewBox=""0 0 16 16"" width=""16"" height=""16"" aria-hidden=""true""><path d=""M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575Zm1.763.707a.25.25 0 0 0-.44 0L1.698 13.132a.25.25 0 0 0 .22.368h12.164a.25.25 0 0 0 .22-.368Zm.53 3.996v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z""></path></svg>",
            "caution" => @"<svg class=""octicon octicon-stop"" viewBox=""0 0 16 16"" width=""16"" height=""16"" aria-hidden=""true""><path d=""M4.47.047A1.75 1.75 0 0 1 5.71 0h4.58c.464 0 .909.184 1.237.513l4.96 4.96c.329.328.513.773.513 1.237v4.58c0 .464-.184.909-.513 1.237l-4.96 4.96a1.75 1.75 0 0 1-1.237.513H5.71a1.75 1.75 0 0 1-1.237-.513l-4.96-4.96A1.75 1.75 0 0 1 0 11.29V6.71c0-.464.184-.909.513-1.237l4.96-4.96Z""></path></svg>",
            _ => @"<svg class=""octicon octicon-info"" viewBox=""0 0 16 16"" width=""16"" height=""16"" aria-hidden=""true""><path d=""M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.5 7.75A.75.75 0 0 1 7.25 7h1a.75.75 0 0 1 .75.75v2.75h.25a.75.75 0 0 1 0 1.5h-2a.75.75 0 0 1 0-1.5h.25v-2h-.25a.75.75 0 0 1-.75-.75ZM8 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z""></path></svg>"
        };
    }
}
