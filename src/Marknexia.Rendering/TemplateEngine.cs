using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using Marknexia.Core;

namespace Marknexia.Rendering;

public sealed class TemplateEngine
{
    private static readonly Lazy<string> CachedCss = new(() => LoadEmbeddedResource("github-markdown.css"));
    private static readonly Lazy<string> CachedBridgeJs = new(() => LoadEmbeddedResource("bridge.js"));
    private static readonly Lazy<string> CachedMermaidJs = new(() => LoadEmbeddedResource("mermaid.min.js"));

    public string GenerateHtml(string bodyHtml, RenderContext context, bool hasMermaid, DocumentAssetContext? assetContext = null)
    {
        assetContext ??= DocumentAssetContext.Create(context.SourcePath, context.RepositoryRoot);
        string themeAttr = context.Theme switch
        {
            AppTheme.Dark => "dark",
            AppTheme.Light => "light",
            _ => "system"
        };
        string nonce = Convert.ToBase64String(RandomNumberGenerator.GetBytes(16));
        string remoteImageSources = context.AllowRemoteAssets ? " http: https:" : string.Empty;
        string contentSecurityPolicy = string.Join("; ",
            "default-src 'none'",
            $"base-uri {assetContext.Origin}",
            $"script-src 'nonce-{nonce}' https://marknexia.assets",
            "style-src 'unsafe-inline'",
            $"img-src {assetContext.Origin} data:{remoteImageSources}",
            "font-src 'none'",
            "object-src 'none'",
            "frame-src 'none'",
            "connect-src 'none'",
            "form-action 'none'");

        var sb = new StringBuilder();
        sb.AppendLine("<!DOCTYPE html>");
        sb.AppendLine($"<html lang=\"en\" data-theme=\"{themeAttr}\">");
        sb.AppendLine("<head>");
        sb.AppendLine("  <meta charset=\"utf-8\" />");
        sb.AppendLine("  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\" />");
        sb.AppendLine($"  <base href=\"{System.Net.WebUtility.HtmlEncode(assetContext.BaseUri.AbsoluteUri)}\" />");
        sb.AppendLine($"  <meta http-equiv=\"Content-Security-Policy\" content=\"{System.Net.WebUtility.HtmlEncode(contentSecurityPolicy)}\" />");
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
            sb.AppendLine($"  <script src=\"https://marknexia.assets/mermaid.min.js\" nonce=\"{nonce}\"></script>");
        }

        sb.AppendLine($"  <script nonce=\"{nonce}\">");
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
                using var resourceCopy = new MemoryStream();
                stream.CopyTo(resourceCopy);
                byte[] expectedBytes = resourceCopy.ToArray();
                string expectedHash = Convert.ToHexString(SHA256.HashData(expectedBytes));
                bool shouldReplace = true;
                if (File.Exists(mermaidPath))
                {
                    try
                    {
                        FileAttributes attributes = File.GetAttributes(mermaidPath);
                        shouldReplace = (attributes & FileAttributes.ReparsePoint) != 0
                            || !Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(mermaidPath)))
                                .Equals(expectedHash, StringComparison.OrdinalIgnoreCase);
                    }
                    catch
                    {
                        shouldReplace = true;
                    }
                }

                if (shouldReplace)
                {
                    string temporaryPath = $"{mermaidPath}.{Guid.NewGuid():N}.tmp";
                    try
                    {
                        File.WriteAllBytes(temporaryPath, expectedBytes);
                        File.Move(temporaryPath, mermaidPath, overwrite: true);
                    }
                    finally
                    {
                        try { if (File.Exists(temporaryPath)) File.Delete(temporaryPath); }
                        catch { }
                    }
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
