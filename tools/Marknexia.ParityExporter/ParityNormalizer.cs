using System.Text.RegularExpressions;

namespace Marknexia.ParityExporter;

public static partial class ParityNormalizer
{
    public static string NormalizeHtml(string html)
    {
        string normalized = NewLines().Replace(html ?? string.Empty, "\n");
        Match document = GeneratedDocument().Match(normalized);
        if (!document.Success) return normalized;

        string host = document.Groups["host"].Value;
        string nonce = document.Groups["nonce"].Value;
        normalized = GeneratedBaseHost(host).Replace(normalized, "document-{volatile}.marknexia.viewer");
        normalized = GeneratedCspHost(host).Replace(normalized, "document-{volatile}.marknexia.viewer");
        normalized = Regex.Replace(normalized, $"(?<=script-src &#39;nonce-){Regex.Escape(nonce)}(?=&#39;)", "{volatile}");
        normalized = GeneratedMermaidNonce(nonce).Replace(normalized, "{volatile}");
        return GeneratedScriptNonce(nonce).Replace(normalized, "<script nonce=\"{volatile}\">\nwindow.marknexiaBridge =");
    }

    [GeneratedRegex("\\r\\n?|\\n")]
    private static partial Regex NewLines();

    // Restrict replacement to TemplateEngine's generated base/CSP/bridge structure.
    // User text, URL values, and independently authored nonce attributes remain intact.
    [GeneratedRegex("<!DOCTYPE html>[\\s\\S]*?<base href=\"https://(?<host>document-[0-9a-f]{32}\\.marknexia\\.viewer)/compat/\"[\\s\\S]*?<script nonce=\"(?<nonce>[A-Za-z0-9+/=]+)\">\\nwindow\\.marknexiaBridge =", RegexOptions.IgnoreCase)]
    private static partial Regex GeneratedDocument();

    private static Regex GeneratedBaseHost(string host) => new($"(?<=<base href=\"https://){Regex.Escape(host)}(?=/compat/\")", RegexOptions.CultureInvariant);

    private static Regex GeneratedCspHost(string host) => new($"(?<=(?:base-uri|img-src) https://){Regex.Escape(host)}(?=[ ;])", RegexOptions.CultureInvariant);

    private static Regex GeneratedScriptNonce(string nonce) => new($"<script nonce=\"{Regex.Escape(nonce)}\">\\nwindow\\.marknexiaBridge =", RegexOptions.CultureInvariant);

    private static Regex GeneratedMermaidNonce(string nonce) => new($"(?<=<script src=\"https://marknexia.assets/mermaid.min.js\" nonce=\"){Regex.Escape(nonce)}(?=\"></script>)", RegexOptions.CultureInvariant);
}
