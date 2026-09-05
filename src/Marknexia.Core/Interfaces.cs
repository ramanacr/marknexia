namespace Marknexia.Core;

public interface INavigationResolver
{
    NavigationIntent Resolve(string? destination, ResolutionContext context);
    UriClassification Classify(string? destination);
}

public interface IPathCanonicalizer
{
    string CanonicalizePath(string path);
    bool IsWithinRoot(string candidatePath, string rootPath);
    string GetRelativePath(string relativeTo, string path);
}

public interface IFileService
{
    Task<FileReadResult> ReadTextAsync(string filePath, CancellationToken cancellationToken = default);
    bool FileExists(string filePath);
    bool DirectoryExists(string directoryPath);
    string ComputeContentHash(string content);
}

public interface IRepositoryService
{
    string? DetectRepositoryRoot(string filePath);
    RepositoryContext? CreateContext(string rootPath, bool enforceSandbox = true);
    IReadOnlyList<string> EnumerateWorkspaceFiles(string rootPath, bool showHidden = false);
}

public interface IMarkdownParserAdapter
{
    ParsedMarkdown Parse(string markdownSource);
}

public interface IHtmlSanitizer
{
    string SanitizeHtml(string rawHtml);
    string SanitizeSvg(string rawSvg);
}

public interface ISyntaxHighlighter
{
    string HighlightCode(string code, string? language);
}

public interface IDiagramRenderer
{
    string DiagramType { get; }
    string RenderDiagramToHtml(string diagramSource, string diagramId);
}

public interface IMarkdownRenderer
{
    Task<RenderedDocument> RenderAsync(string markdownSource, RenderContext context, CancellationToken cancellationToken = default);
}

public interface INavigationHistoryManager
{
    bool CanGoBack { get; }
    bool CanGoForward { get; }
    void Push(HistoryEntry entry);
    HistoryEntry? GoBack(HistoryEntry currentEntry);
    HistoryEntry? GoForward(HistoryEntry currentEntry);
    void Clear();
    IReadOnlyList<HistoryEntry> BackStack { get; }
    IReadOnlyList<HistoryEntry> ForwardStack { get; }
}
