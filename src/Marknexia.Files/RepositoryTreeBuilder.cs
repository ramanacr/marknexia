using Marknexia.Core;

namespace Marknexia.Files;

public sealed class RepositoryTreeBuilder
{
    private static readonly HashSet<string> SupportedExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".md", ".markdown", ".mdown", ".mkdn", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".bmp", ".pdf"
    };

    private static readonly HashSet<string> IgnoredDirectories = new(StringComparer.OrdinalIgnoreCase)
    {
        ".git", ".vs", "bin", "obj", "node_modules", ".idea", ".vscode",
        ".codegraph", "artifacts", "dist", "publish", "TestResults", "coverage"
    };

    private readonly IPathCanonicalizer _canonicalizer;

    public static bool IsSupportedMarkdownFile(string path)
    {
        string extension = Path.GetExtension(path);
        return extension.Equals(".md", StringComparison.OrdinalIgnoreCase)
            || extension.Equals(".markdown", StringComparison.OrdinalIgnoreCase)
            || extension.Equals(".mdown", StringComparison.OrdinalIgnoreCase)
            || extension.Equals(".mkdn", StringComparison.OrdinalIgnoreCase);
    }

    public RepositoryTreeBuilder(IPathCanonicalizer canonicalizer)
    {
        _canonicalizer = canonicalizer ?? throw new ArgumentNullException(nameof(canonicalizer));
    }

    public RepositoryTreeNode Build(string rootPath)
        => BuildCore(rootPath, CancellationToken.None);

    public Task<RepositoryTreeNode> BuildAsync(string rootPath, CancellationToken cancellationToken = default)
    {
        cancellationToken.ThrowIfCancellationRequested();
        return Task.Run(() => BuildCore(rootPath, cancellationToken), cancellationToken);
    }

    private RepositoryTreeNode BuildCore(string rootPath, CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        string root = _canonicalizer.CanonicalizePath(rootPath);
        if (!Directory.Exists(root)) return new RepositoryTreeNode(Path.GetFileName(root) ?? root, root, true);
        return BuildFolder(new DirectoryInfo(root), root, cancellationToken);
    }

    private RepositoryTreeNode BuildFolder(DirectoryInfo directory, string root, CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        var children = new List<RepositoryTreeNode>();
        foreach (DirectoryInfo child in SafeDirectories(directory))
        {
            cancellationToken.ThrowIfCancellationRequested();
            if (IgnoredDirectories.Contains(child.Name) || !_canonicalizer.IsWithinRoot(child.FullName, root)) continue;
            RepositoryTreeNode node = BuildFolder(child, root, cancellationToken);
            if (node.Children.Count > 0) children.Add(node);
        }

        foreach (FileInfo file in SafeFiles(directory))
        {
            cancellationToken.ThrowIfCancellationRequested();
            if (SupportedExtensions.Contains(file.Extension) && _canonicalizer.IsWithinRoot(file.FullName, root))
                children.Add(new RepositoryTreeNode(file.Name, file.FullName, false));
        }

        return new RepositoryTreeNode(directory.Name, directory.FullName, true,
            children.OrderByDescending(x => x.IsFolder).ThenBy(x => x.DisplayName, StringComparer.OrdinalIgnoreCase).ToArray());
    }

    private static IEnumerable<DirectoryInfo> SafeDirectories(DirectoryInfo directory)
    {
        try
        {
            return directory.EnumerateDirectories()
                .Where(d => (d.Attributes & (FileAttributes.Hidden | FileAttributes.ReparsePoint)) == 0)
                .ToArray();
        }
        catch (UnauthorizedAccessException) { return Array.Empty<DirectoryInfo>(); }
    }

    private static IEnumerable<FileInfo> SafeFiles(DirectoryInfo directory)
    {
        try
        {
            return directory.EnumerateFiles()
                .Where(f => (f.Attributes & (FileAttributes.Hidden | FileAttributes.ReparsePoint)) == 0)
                .ToArray();
        }
        catch (UnauthorizedAccessException) { return Array.Empty<FileInfo>(); }
    }
}
