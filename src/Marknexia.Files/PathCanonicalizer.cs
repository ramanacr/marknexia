using System.Security;
using Marknexia.Core;

namespace Marknexia.Files;

public sealed class PathCanonicalizer : IPathCanonicalizer
{
    public string CanonicalizePath(string path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            return string.Empty;
        }

        // Handle file URI schemes if passed
        if (path.StartsWith("file:///", StringComparison.OrdinalIgnoreCase))
        {
            path = Uri.UnescapeDataString(path[8..]);
            path = path.Replace('/', '\\');
        }
        else if (path.StartsWith("file://", StringComparison.OrdinalIgnoreCase))
        {
            path = Uri.UnescapeDataString(path[7..]);
            path = path.Replace('/', '\\');
        }

        try
        {
            string fullPath = Path.GetFullPath(path);
            return NormalizePathSeparators(fullPath);
        }
        catch (Exception ex) when (ex is ArgumentException or NotSupportedException or PathTooLongException or SecurityException)
        {
            return path.Replace('/', '\\').TrimEnd('\\');
        }
    }

    public bool IsWithinRoot(string candidatePath, string rootPath)
    {
        if (string.IsNullOrWhiteSpace(candidatePath) || string.IsNullOrWhiteSpace(rootPath))
        {
            return false;
        }

        string canonCandidate = CanonicalizePath(candidatePath);
        string canonRoot = CanonicalizePath(rootPath);

        if (!canonRoot.EndsWith('\\'))
        {
            canonRoot += '\\';
        }

        if (canonCandidate.Equals(canonRoot.TrimEnd('\\'), StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        return canonCandidate.StartsWith(canonRoot, StringComparison.OrdinalIgnoreCase);
    }

    public string GetRelativePath(string relativeTo, string path)
    {
        string canonFrom = CanonicalizePath(relativeTo);
        string canonTo = CanonicalizePath(path);

        if (File.Exists(canonFrom))
        {
            canonFrom = Path.GetDirectoryName(canonFrom) ?? canonFrom;
        }

        return Path.GetRelativePath(canonFrom, canonTo).Replace('\\', '/');
    }

    private static string NormalizePathSeparators(string path)
    {
        string normalized = path.Replace('/', '\\');
        string root = Path.GetPathRoot(normalized) ?? string.Empty;
        if (normalized.Length > root.Length && normalized.EndsWith('\\'))
        {
            normalized = normalized.TrimEnd('\\');
        }
        return normalized;
    }
}
