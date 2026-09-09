using Marknexia.Core;

namespace Marknexia.Files;

public sealed class RepositoryService : IRepositoryService
{
    private readonly IPathCanonicalizer _pathCanonicalizer;

    private static readonly HashSet<string> MarkdownExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".md", ".markdown", ".mdown", ".mkdn"
    };

    private static readonly HashSet<string> AssetExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".bmp", ".pdf"
    };

    private static readonly HashSet<string> IgnoredDirectories = new(StringComparer.OrdinalIgnoreCase)
    {
        ".git", ".vs", "bin", "obj", "node_modules", ".idea", ".vscode"
    };

    public RepositoryService(IPathCanonicalizer pathCanonicalizer)
    {
        _pathCanonicalizer = pathCanonicalizer ?? throw new ArgumentNullException(nameof(pathCanonicalizer));
    }

    public string? DetectRepositoryRoot(string filePath)
    {
        if (string.IsNullOrWhiteSpace(filePath)) return null;

        string canonical = _pathCanonicalizer.CanonicalizePath(filePath);
        string? currentDir = File.Exists(canonical) ? Path.GetDirectoryName(canonical) : canonical;

        while (!string.IsNullOrEmpty(currentDir) && Directory.Exists(currentDir))
        {
            if (Directory.Exists(Path.Combine(currentDir, ".git")) || File.Exists(Path.Combine(currentDir, ".git")))
            {
                return currentDir;
            }

            // Check for solution root as marker
            if (Directory.EnumerateFiles(currentDir, "*.sln").Any() || Directory.EnumerateFiles(currentDir, "*.slnx").Any())
            {
                return currentDir;
            }

            DirectoryInfo? parent = Directory.GetParent(currentDir);
            if (parent == null || parent.FullName.Equals(currentDir, StringComparison.OrdinalIgnoreCase))
            {
                break;
            }
            currentDir = parent.FullName;
        }

        return null;
    }

    public RepositoryContext? CreateContext(string rootPath, bool enforceSandbox = true)
    {
        if (string.IsNullOrWhiteSpace(rootPath)) return null;

        string canonical = _pathCanonicalizer.CanonicalizePath(rootPath);
        if (!Directory.Exists(canonical)) return null;

        bool hasGit = Directory.Exists(Path.Combine(canonical, ".git")) || File.Exists(Path.Combine(canonical, ".git"));
        return new RepositoryContext(
            canonical,
            hasGit ? RepositoryKind.GitRepository : RepositoryKind.Folder,
            enforceSandbox);
    }

    public IReadOnlyList<string> EnumerateWorkspaceFiles(string rootPath, bool showHidden = false)
    {
        if (string.IsNullOrWhiteSpace(rootPath)) return Array.Empty<string>();

        string canonical = _pathCanonicalizer.CanonicalizePath(rootPath);
        if (!Directory.Exists(canonical)) return Array.Empty<string>();

        var results = new List<string>();
        var rootDirInfo = new DirectoryInfo(canonical);

        TraverseDirectory(rootDirInfo, results, showHidden);
        return results;
    }

    private static void TraverseDirectory(DirectoryInfo dir, List<string> results, bool showHidden)
    {
        try
        {
            foreach (FileInfo file in dir.EnumerateFiles())
            {
                if ((file.Attributes & FileAttributes.ReparsePoint) != 0
                    || (!showHidden && (file.Attributes & FileAttributes.Hidden) != 0))
                {
                    continue;
                }

                string ext = file.Extension;
                if (MarkdownExtensions.Contains(ext) || AssetExtensions.Contains(ext))
                {
                    results.Add(file.FullName);
                }
            }

            foreach (DirectoryInfo subDir in dir.EnumerateDirectories())
            {
                if ((subDir.Attributes & FileAttributes.ReparsePoint) != 0
                    || (!showHidden && (subDir.Attributes & FileAttributes.Hidden) != 0))
                {
                    continue;
                }

                if (IgnoredDirectories.Contains(subDir.Name))
                {
                    continue;
                }

                TraverseDirectory(subDir, results, showHidden);
            }
        }
        catch (UnauthorizedAccessException)
        {
            // Fail soft on protected directories
        }
    }
}
