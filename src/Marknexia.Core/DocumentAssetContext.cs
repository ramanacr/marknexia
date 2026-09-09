namespace Marknexia.Core;

/// <summary>Immutable asset authority and URL base for one rendered document.</summary>
public sealed record DocumentAssetContext
{
    public string RootPath { get; }
    public Uri BaseUri { get; }
    public string Origin => BaseUri.GetLeftPart(UriPartial.Authority);

    private DocumentAssetContext(string rootPath, Uri baseUri)
    {
        RootPath = rootPath;
        BaseUri = baseUri;
    }

    public static DocumentAssetContext Create(string sourcePath, string? repositoryRoot)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(sourcePath);
        string source = Path.GetFullPath(sourcePath);
        string directory = Path.GetDirectoryName(source) ?? throw new ArgumentException("A document directory is required.", nameof(sourcePath));
        // Standalone documents have no repository manifest that can declare an
        // asset root. Authorize the immediate parent so a sibling asset reached
        // with one `../` segment can load, while keeping all reads bounded.
        string standaloneRoot = Path.GetDirectoryName(directory) ?? directory;
        string root = Path.TrimEndingDirectorySeparator(Path.GetFullPath(repositoryRoot ?? standaloneRoot));
        string relativeDirectory = Path.GetRelativePath(root, directory);
        if (Path.IsPathRooted(relativeDirectory) || relativeDirectory == ".."
            || relativeDirectory.StartsWith(".." + Path.DirectorySeparatorChar, StringComparison.Ordinal))
            throw new ArgumentException("The document is outside its authorized asset root.", nameof(repositoryRoot));

        string path = relativeDirectory == "." ? string.Empty
            : string.Join('/', relativeDirectory.Replace('\\', '/').Split('/').Select(Uri.EscapeDataString)) + "/";
        var basis = new Uri($"https://document-{Guid.NewGuid():N}.marknexia.viewer/{path}");
        return new DocumentAssetContext(root, basis);
    }
}
