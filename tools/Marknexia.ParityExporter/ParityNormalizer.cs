using System.Text.RegularExpressions;

namespace Marknexia.ParityExporter;

public static partial class ParityNormalizer
{
    public static string NormalizeHtml(string html)
    {
        string normalized = NewLines().Replace(html ?? string.Empty, "\n");
        normalized = VirtualDocumentHost().Replace(normalized, "document-{volatile}.marknexia.viewer");
        normalized = CspNonce().Replace(normalized, "nonce-{volatile}");
        return ScriptNonce().Replace(normalized, "{volatile}");
    }

    [GeneratedRegex("\\r\\n?|\\n")]
    private static partial Regex NewLines();

    // DocumentAssetContext creates a per-render virtual host and CSP nonce. They are
    // transport isolation values, not observable document content, so the fixture
    // records stable placeholders while retaining every element and URL shape.
    [GeneratedRegex("document-[0-9a-f]{32}\\.marknexia\\.viewer", RegexOptions.IgnoreCase)]
    private static partial Regex VirtualDocumentHost();

    [GeneratedRegex("nonce-[A-Za-z0-9+/=]+")]
    private static partial Regex CspNonce();

    [GeneratedRegex("(?<=nonce=\")[A-Za-z0-9+/=]+")]
    private static partial Regex ScriptNonce();
}
