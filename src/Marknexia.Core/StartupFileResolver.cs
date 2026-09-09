namespace Marknexia.Core;

/// <summary>
/// Resolves the first usable Markdown file candidate supplied by Windows
/// activation, command-line launching, or a direct executable invocation.
/// </summary>
public static class StartupFileResolver
{
    public static string? FindFirstExisting(IEnumerable<string?> candidates)
    {
        foreach (string? candidate in candidates)
        {
            if (string.IsNullOrWhiteSpace(candidate))
            {
                continue;
            }

            string path = candidate.Trim().Trim('"');
            if (path.Length == 0)
            {
                continue;
            }

            try
            {
                string fullPath = Path.GetFullPath(path);
                if (File.Exists(fullPath))
                {
                    return fullPath;
                }
            }
            catch (ArgumentException)
            {
                // An invalid activation argument should not prevent the app
                // shell from opening normally.
            }
            catch (NotSupportedException)
            {
                // Some activation payloads can contain unsupported path forms.
            }
            catch (PathTooLongException)
            {
                // Ignore unusable paths while continuing through candidates.
            }
        }

        return null;
    }
}
