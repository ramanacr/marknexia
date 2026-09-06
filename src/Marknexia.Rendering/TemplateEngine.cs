using System.Reflection;
using System.Text;
using Marknexia.Core;

namespace Marknexia.Rendering;

public sealed class TemplateEngine
{
    private static readonly Lazy<string> CachedCss = new(() => LoadEmbeddedResource("github-markdown.css"));
    private static readonly Lazy<string> CachedBridgeJs = new(() => LoadEmbeddedResource("bridge.js"));
    private static readonly Lazy<string> CachedMermaidJs = new(() => LoadEmbeddedResource("mermaid.min.js"));

    public string GenerateHtml(string bodyHtml, RenderContext context, bool hasMermaid)
    {
        string themeAttr = context.Theme switch
        {
            AppTheme.Dark => "dark",
            AppTheme.Light => "light",
            _ => "light"
        };

        var sb = new StringBuilder();
        sb.AppendLine("<!DOCTYPE html>");
        sb.AppendLine($"<html lang=\"en\" data-theme=\"{themeAttr}\">");
        sb.AppendLine("<head>");
        sb.AppendLine("  <meta charset=\"utf-8\" />");
        sb.AppendLine("  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\" />");
        sb.AppendLine("  <base href=\"https://marknexia.viewer/\" />");
        sb.AppendLine("  <style>");
        sb.AppendLine(CachedCss.Value);
        sb.AppendLine("  </style>");
        sb.AppendLine("</head>");
        sb.AppendLine("<body>");
        sb.AppendLine("  <div class=\"markdown-body\">");
        sb.AppendLine(bodyHtml);
        sb.AppendLine("  </div>");

        if (hasMermaid && context.EnableDiagrams)
        {
            sb.AppendLine("  <script src=\"https://marknexia.assets/mermaid.min.js\"></script>");
        }

        sb.AppendLine("  <script>");
        sb.AppendLine(CachedBridgeJs.Value);
        sb.AppendLine("  </script>");
        sb.AppendLine("</body>");
        sb.AppendLine("</html>");

        return sb.ToString();
    }

    public static void EnsureAssetsExtracted(string destinationDirectory)
    {
        Directory.CreateDirectory(destinationDirectory);
        string mermaidPath = Path.Combine(destinationDirectory, "mermaid.min.js");

        var assembly = typeof(TemplateEngine).Assembly;
        string? resourceName = assembly.GetManifestResourceNames()
            .FirstOrDefault(n => n.EndsWith("mermaid.min.js", StringComparison.OrdinalIgnoreCase));

        if (resourceName != null)
        {
            using Stream? stream = assembly.GetManifestResourceStream(resourceName);
            if (stream != null)
            {
                if (!File.Exists(mermaidPath) || new FileInfo(mermaidPath).Length != stream.Length)
                {
                    using var fs = new FileStream(mermaidPath, FileMode.Create, FileAccess.Write);
                    stream.CopyTo(fs);
                }
            }
        }
    }

    private static string LoadEmbeddedResource(string fileName)
    {
        var assembly = typeof(TemplateEngine).Assembly;
        string resourceName = assembly.GetManifestResourceNames()
            .FirstOrDefault(n => n.EndsWith(fileName, StringComparison.OrdinalIgnoreCase))
            ?? throw new FileNotFoundException($"Embedded resource not found: {fileName}");

        using Stream? stream = assembly.GetManifestResourceStream(resourceName);
        if (stream == null)
        {
            throw new InvalidOperationException($"Could not open stream for embedded resource: {resourceName}");
        }

        using var reader = new StreamReader(stream, Encoding.UTF8);
        return reader.ReadToEnd();
    }
}
