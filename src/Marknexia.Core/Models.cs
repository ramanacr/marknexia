namespace Marknexia.Core;

public enum NavigationKind
{
    SameDocumentAnchor,
    CrossDocument,
    CrossDocumentWithAnchor,
    ExternalBrowser,
    RepositoryRootRelative,
    BlockedOrInvalid,
    BrokenTarget
}

public enum UriClassification
{
    Empty,
    FragmentOnly,
    RelativePath,
    RepositoryRootPath,
    AbsoluteLocalPath,
    Http,
    Https,
    OtherExternal,
    Unsupported
}

public enum AppTheme
{
    System,
    Light,
    Dark
}

public enum DiagnosticSeverity
{
    Info,
    Warning,
    Error
}

public enum RepositoryKind
{
    Folder,
    GitRepository
}

public sealed record DocumentUri(string CanonicalPath, string? Fragment = null)
{
    public override string ToString() =>
        string.IsNullOrEmpty(Fragment) ? CanonicalPath : $"{CanonicalPath}#{Fragment}";
}

public sealed record NavigationIntent(
    NavigationKind Kind,
    DocumentUri? TargetDocument,
    string? Fragment,
    Uri? ExternalUri,
    bool IsSafe,
    string? Diagnostic = null);

public sealed record NavigationPolicy(
    bool EnforceRepositorySandbox = true,
    bool AllowExternalLinks = true,
    bool AllowRemoteAssets = false);

public sealed record ResolutionContext(
    string CurrentFilePath,
    string? RepositoryRoot,
    string? CurrentDocumentUri,
    NavigationPolicy Policy);

public sealed record HistoryEntry(
    DocumentUri Document,
    string? Fragment,
    double ScrollTop,
    DateTimeOffset CreatedAt);

public sealed record HeadingInfo(
    string Text,
    int Level,
    string SlugId,
    int LineNumber = 0);

public sealed record AnchorTarget(
    string Id,
    string Name,
    bool IsHeadingAnchor,
    int LineNumber = 0);

public sealed record DiagnosticInfo(
    DiagnosticSeverity Severity,
    string Message,
    string? SourceSpan = null);

public sealed record RepositoryContext(
    string RootPath,
    RepositoryKind Kind,
    bool EnforceRootSandbox);

public sealed record RenderContext(
    string SourcePath,
    string? RepositoryRoot,
    AppTheme Theme,
    bool EnableDiagrams = true,
    bool EnableMath = true,
    bool AllowRemoteAssets = false);

public sealed record DiagramBlock(
    string Id,
    string DiagramType,
    string SourceCode,
    int LineNumber = 0);

public sealed record ParsedMarkdown(
    string RenderedBodyHtml,
    IReadOnlyList<HeadingInfo> Headings,
    IReadOnlyList<AnchorTarget> CustomAnchors,
    IReadOnlyList<string> ExtractedLinks,
    IReadOnlyList<string> ExtractedImages,
    IReadOnlyList<DiagramBlock> DiagramBlocks,
    IReadOnlyList<DiagnosticInfo> Diagnostics);

public sealed record RenderedDocument(
    string HtmlContent,
    IReadOnlyList<HeadingInfo> Headings,
    IReadOnlyDictionary<string, AnchorTarget> AnchorIndex,
    IReadOnlyList<string> AssetReferences,
    IReadOnlyList<DiagnosticInfo> Diagnostics);

public sealed record FileReadResult(
    string Content,
    string EncodingName,
    string ContentHash,
    long SizeBytes);
