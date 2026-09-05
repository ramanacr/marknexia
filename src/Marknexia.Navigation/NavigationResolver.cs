using Marknexia.Core;

namespace Marknexia.Navigation;

public sealed class NavigationResolver : INavigationResolver
{
    private readonly IPathCanonicalizer _pathCanonicalizer;
    private readonly IFileService _fileService;

    public NavigationResolver(IPathCanonicalizer pathCanonicalizer, IFileService fileService)
    {
        _pathCanonicalizer = pathCanonicalizer ?? throw new ArgumentNullException(nameof(pathCanonicalizer));
        _fileService = fileService ?? throw new ArgumentNullException(nameof(fileService));
    }

    public UriClassification Classify(string? destination)
    {
        if (string.IsNullOrWhiteSpace(destination))
        {
            return UriClassification.Empty;
        }

        string trimmed = destination.Trim();

        if (trimmed.StartsWith('#'))
        {
            return UriClassification.FragmentOnly;
        }

        if (trimmed.StartsWith("javascript:", StringComparison.OrdinalIgnoreCase) ||
            trimmed.StartsWith("vbscript:", StringComparison.OrdinalIgnoreCase) ||
            trimmed.StartsWith("data:", StringComparison.OrdinalIgnoreCase))
        {
            return UriClassification.Unsupported;
        }

        if (trimmed.StartsWith("http://", StringComparison.OrdinalIgnoreCase))
        {
            return UriClassification.Http;
        }

        if (trimmed.StartsWith("https://", StringComparison.OrdinalIgnoreCase))
        {
            return UriClassification.Https;
        }

        if (trimmed.StartsWith("mailto:", StringComparison.OrdinalIgnoreCase) ||
            trimmed.StartsWith("ftp://", StringComparison.OrdinalIgnoreCase) ||
            trimmed.StartsWith("tel:", StringComparison.OrdinalIgnoreCase))
        {
            return UriClassification.OtherExternal;
        }

        if (trimmed.StartsWith('/') || trimmed.StartsWith('\\'))
        {
            return UriClassification.RepositoryRootPath;
        }

        if (HasDriveLetter(trimmed) || trimmed.StartsWith("file://", StringComparison.OrdinalIgnoreCase))
        {
            return UriClassification.AbsoluteLocalPath;
        }

        return UriClassification.RelativePath;
    }

    public NavigationIntent Resolve(string? destination, ResolutionContext context)
    {
        if (context == null) throw new ArgumentNullException(nameof(context));

        UriClassification classification = Classify(destination);

        switch (classification)
        {
            case UriClassification.Empty:
                return new NavigationIntent(NavigationKind.BlockedOrInvalid, null, null, null, false, "Empty destination.");

            case UriClassification.Unsupported:
                return new NavigationIntent(NavigationKind.BlockedOrInvalid, null, null, null, false, "Unsupported or unsafe protocol.");

            case UriClassification.Http:
            case UriClassification.Https:
            case UriClassification.OtherExternal:
                if (!context.Policy.AllowExternalLinks)
                {
                    return new NavigationIntent(NavigationKind.BlockedOrInvalid, null, null, null, false, "External links are disabled by policy.");
                }

                if (Uri.TryCreate(destination, UriKind.Absolute, out Uri? externalUri))
                {
                    return new NavigationIntent(NavigationKind.ExternalBrowser, null, null, externalUri, true);
                }
                return new NavigationIntent(NavigationKind.BlockedOrInvalid, null, null, null, false, "Invalid external URI.");

            case UriClassification.FragmentOnly:
                string fragment = destination!.TrimStart('#');
                string currentDocPath = _pathCanonicalizer.CanonicalizePath(context.CurrentFilePath);
                var currentDocUri = new DocumentUri(currentDocPath, fragment);
                return new NavigationIntent(NavigationKind.SameDocumentAnchor, currentDocUri, fragment, null, true);

            case UriClassification.RepositoryRootPath:
                return ResolveRepositoryRootPath(destination!, context);

            case UriClassification.RelativePath:
                return ResolveRelativePath(destination!, context);

            case UriClassification.AbsoluteLocalPath:
                return ResolveAbsoluteLocalPath(destination!, context);

            default:
                return new NavigationIntent(NavigationKind.BlockedOrInvalid, null, null, null, false, "Unknown destination classification.");
        }
    }

    private NavigationIntent ResolveRepositoryRootPath(string destination, ResolutionContext context)
    {
        var (pathPart, fragment) = SplitPathAndFragment(destination);

        if (string.IsNullOrWhiteSpace(context.RepositoryRoot))
        {
            return new NavigationIntent(
                NavigationKind.BrokenTarget,
                null,
                fragment,
                null,
                false,
                "Cannot resolve repository-root path ('/...'): No active repository context.");
        }

        string trimmedRel = pathPart.TrimStart('/', '\\');
        string combined = Path.Combine(context.RepositoryRoot, trimmedRel);
        string canonicalTarget = _pathCanonicalizer.CanonicalizePath(combined);

        // Security traversal check
        if (context.Policy.EnforceRepositorySandbox && !_pathCanonicalizer.IsWithinRoot(canonicalTarget, context.RepositoryRoot))
        {
            return new NavigationIntent(
                NavigationKind.BlockedOrInvalid,
                null,
                fragment,
                null,
                false,
                "Access blocked: Repository root traversal outside sandbox boundary.");
        }

        bool exists = _fileService.FileExists(canonicalTarget);
        var targetDoc = new DocumentUri(canonicalTarget, fragment);

        if (!exists)
        {
            return new NavigationIntent(
                NavigationKind.BrokenTarget,
                targetDoc,
                fragment,
                null,
                true,
                $"Repository-relative file not found: {canonicalTarget}");
        }

        NavigationKind kind = string.IsNullOrEmpty(fragment)
            ? NavigationKind.CrossDocument
            : NavigationKind.CrossDocumentWithAnchor;

        return new NavigationIntent(kind, targetDoc, fragment, null, true);
    }

    private NavigationIntent ResolveRelativePath(string destination, ResolutionContext context)
    {
        var (pathPart, fragment) = SplitPathAndFragment(destination);

        string currentFile = _pathCanonicalizer.CanonicalizePath(context.CurrentFilePath);
        string? baseDir = _fileService.FileExists(currentFile) ? Path.GetDirectoryName(currentFile) : currentFile;

        if (string.IsNullOrEmpty(baseDir))
        {
            baseDir = Directory.GetCurrentDirectory();
        }

        string combined = Path.Combine(baseDir, pathPart);
        string canonicalTarget = _pathCanonicalizer.CanonicalizePath(combined);

        // Security check: if repository sandboxing is enforced and root is known, check root escape
        if (context.Policy.EnforceRepositorySandbox && !string.IsNullOrWhiteSpace(context.RepositoryRoot))
        {
            if (!_pathCanonicalizer.IsWithinRoot(canonicalTarget, context.RepositoryRoot))
            {
                return new NavigationIntent(
                    NavigationKind.BlockedOrInvalid,
                    null,
                    fragment,
                    null,
                    false,
                    "Access blocked: Relative path escapes repository root sandbox.");
            }
        }

        bool exists = _fileService.FileExists(canonicalTarget);
        var targetDoc = new DocumentUri(canonicalTarget, fragment);

        if (!exists)
        {
            return new NavigationIntent(
                NavigationKind.BrokenTarget,
                targetDoc,
                fragment,
                null,
                true,
                $"Target file not found: {canonicalTarget}");
        }

        NavigationKind kind = string.IsNullOrEmpty(fragment)
            ? NavigationKind.CrossDocument
            : NavigationKind.CrossDocumentWithAnchor;

        return new NavigationIntent(kind, targetDoc, fragment, null, true);
    }

    private NavigationIntent ResolveAbsoluteLocalPath(string destination, ResolutionContext context)
    {
        var (pathPart, fragment) = SplitPathAndFragment(destination);
        string canonicalTarget = _pathCanonicalizer.CanonicalizePath(pathPart);

        if (context.Policy.EnforceRepositorySandbox && !string.IsNullOrWhiteSpace(context.RepositoryRoot))
        {
            if (!_pathCanonicalizer.IsWithinRoot(canonicalTarget, context.RepositoryRoot))
            {
                return new NavigationIntent(
                    NavigationKind.BlockedOrInvalid,
                    null,
                    fragment,
                    null,
                    false,
                    "Access blocked: Absolute path is outside repository sandbox.");
            }
        }

        bool exists = _fileService.FileExists(canonicalTarget);
        var targetDoc = new DocumentUri(canonicalTarget, fragment);

        if (!exists)
        {
            return new NavigationIntent(
                NavigationKind.BrokenTarget,
                targetDoc,
                fragment,
                null,
                true,
                $"Absolute target file not found: {canonicalTarget}");
        }

        NavigationKind kind = string.IsNullOrEmpty(fragment)
            ? NavigationKind.CrossDocument
            : NavigationKind.CrossDocumentWithAnchor;

        return new NavigationIntent(kind, targetDoc, fragment, null, true);
    }

    private static (string PathPart, string? Fragment) SplitPathAndFragment(string destination)
    {
        int hashIndex = destination.IndexOf('#');
        if (hashIndex < 0)
        {
            return (destination, null);
        }

        string pathPart = destination[..hashIndex];
        string fragment = destination[(hashIndex + 1)..];
        return (pathPart, string.IsNullOrEmpty(fragment) ? null : fragment);
    }

    private static bool HasDriveLetter(string path)
    {
        return path.Length >= 2 && char.IsLetter(path[0]) && path[1] == ':';
    }
}
