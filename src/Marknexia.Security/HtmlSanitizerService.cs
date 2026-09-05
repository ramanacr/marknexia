using System.Text.RegularExpressions;
using Ganss.Xss;
using Marknexia.Core;

namespace Marknexia.Security;

public sealed class HtmlSanitizerService : Marknexia.Core.IHtmlSanitizer
{
    private readonly HtmlSanitizer _sanitizer;

    public HtmlSanitizerService()
    {
        _sanitizer = new HtmlSanitizer();

        // Configure allowed tags
        _sanitizer.AllowedTags.Add("div");
        _sanitizer.AllowedTags.Add("span");
        _sanitizer.AllowedTags.Add("details");
        _sanitizer.AllowedTags.Add("summary");
        _sanitizer.AllowedTags.Add("input");
        _sanitizer.AllowedTags.Add("svg");
        _sanitizer.AllowedTags.Add("path");
        _sanitizer.AllowedTags.Add("g");
        _sanitizer.AllowedTags.Add("circle");
        _sanitizer.AllowedTags.Add("rect");
        _sanitizer.AllowedTags.Add("line");
        _sanitizer.AllowedTags.Add("text");
        _sanitizer.AllowedTags.Add("defs");
        _sanitizer.AllowedTags.Add("marker");
        _sanitizer.AllowedTags.Add("polyline");
        _sanitizer.AllowedTags.Add("polygon");
        _sanitizer.AllowedTags.Add("use");

        // Disallow dangerous tags explicitly
        _sanitizer.AllowedTags.Remove("script");
        _sanitizer.AllowedTags.Remove("iframe");
        _sanitizer.AllowedTags.Remove("object");
        _sanitizer.AllowedTags.Remove("embed");
        _sanitizer.AllowedTags.Remove("applet");
        _sanitizer.AllowedTags.Remove("form");
        _sanitizer.AllowedTags.Remove("base");
        _sanitizer.AllowedTags.Remove("meta");
        _sanitizer.AllowedTags.Remove("link");

        // Allowed attributes
        _sanitizer.AllowedAttributes.Add("class");
        _sanitizer.AllowedAttributes.Add("id");
        _sanitizer.AllowedAttributes.Add("name");
        _sanitizer.AllowedAttributes.Add("aria-hidden");
        _sanitizer.AllowedAttributes.Add("viewBox");
        _sanitizer.AllowedAttributes.Add("width");
        _sanitizer.AllowedAttributes.Add("height");
        _sanitizer.AllowedAttributes.Add("fill");
        _sanitizer.AllowedAttributes.Add("stroke");
        _sanitizer.AllowedAttributes.Add("stroke-width");
        _sanitizer.AllowedAttributes.Add("d");
        _sanitizer.AllowedAttributes.Add("type");
        _sanitizer.AllowedAttributes.Add("checked");
        _sanitizer.AllowedAttributes.Add("disabled");

        // URL schemes allowed
        _sanitizer.AllowedSchemes.Clear();
        _sanitizer.AllowedSchemes.Add("http");
        _sanitizer.AllowedSchemes.Add("https");
        _sanitizer.AllowedSchemes.Add("mailto");
        _sanitizer.AllowedSchemes.Add("tel");

        // Event handler removing hook
        _sanitizer.RemovingAttribute += (s, e) =>
        {
            if (e.Attribute.Name.StartsWith("on", StringComparison.OrdinalIgnoreCase))
            {
                e.Cancel = false; // definitely remove
            }
        };
    }

    public string SanitizeHtml(string rawHtml)
    {
        if (string.IsNullOrWhiteSpace(rawHtml))
        {
            return string.Empty;
        }

        // 1. Strip javascript: and vbscript: URIs before parsing
        string preFiltered = Regex.Replace(rawHtml, @"(href|src)\s*=\s*[""']\s*(?:javascript|vbscript):[^""']*[""']", "$1=\"#\"", RegexOptions.IgnoreCase);

        // 2. Strip explicit <script> tags and comments hiding scripts
        preFiltered = Regex.Replace(preFiltered, @"<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>", string.Empty, RegexOptions.IgnoreCase);

        // 3. Ganss HtmlSanitizer sanitization
        string sanitized = _sanitizer.Sanitize(preFiltered);

        return sanitized;
    }

    public string SanitizeSvg(string rawSvg)
    {
        if (string.IsNullOrWhiteSpace(rawSvg))
        {
            return string.Empty;
        }

        // SVG must not contain script tags or event handlers
        string cleaned = Regex.Replace(rawSvg, @"<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>", string.Empty, RegexOptions.IgnoreCase);
        cleaned = Regex.Replace(cleaned, @"\bon\w+\s*=\s*[""'][^""']*[""']", string.Empty, RegexOptions.IgnoreCase);
        cleaned = Regex.Replace(cleaned, @"href\s*=\s*[""']\s*(?:javascript|vbscript):[^""']*[""']", "href=\"#\"", RegexOptions.IgnoreCase);

        return _sanitizer.Sanitize(cleaned);
    }
}
