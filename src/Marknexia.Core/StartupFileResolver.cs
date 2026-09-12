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

            // A StorageFile or already-tokenized argv item can contain spaces.
            // Try it intact before interpreting a raw activation command line.
            if (ExistingMarkdown(candidate.Trim().Trim('"')) is string direct) return direct;
            foreach (string argument in SplitCommandLine(candidate))
                if (ExistingMarkdown(argument) is string path) return path;
        }

        return null;
    }

    private static string? ExistingMarkdown(string path)
    {
        try
        {
            if (Uri.TryCreate(path, UriKind.Absolute, out Uri? uri) && uri.IsFile)
            {
                path = uri.LocalPath;
            }

            string extension = Path.GetExtension(path);
            if (!new[] { ".md", ".markdown", ".mdown", ".mkdn" }
                .Contains(extension, StringComparer.OrdinalIgnoreCase))
            {
                return null;
            }

            string fullPath = Path.GetFullPath(path);
            return File.Exists(fullPath) ? fullPath : null;
        }
        catch (Exception ex) when (ex is ArgumentException or NotSupportedException or PathTooLongException)
        {
            return null;
        }
    }

    // Windows argv rules: whitespace outside quotes separates arguments;
    // backslashes are literal except immediately before a quote (2n/2n+1).
    private static IEnumerable<string> SplitCommandLine(string text)
    {
        var argument = new System.Text.StringBuilder();
        bool quoted = false;
        for (int index = 0; index < text.Length; index++)
        {
            char current = text[index];
            if (current == '\\')
            {
                int start = index;
                while (index < text.Length && text[index] == '\\') index++;
                int count = index - start;
                if (index < text.Length && text[index] == '"')
                {
                    argument.Append('\\', count / 2);
                    if (count % 2 != 0) argument.Append('"');
                    else quoted = !quoted;
                }
                else
                {
                    argument.Append('\\', count);
                    index--;
                }
            }
            else if (current == '"')
            {
                quoted = !quoted;
            }
            else if (char.IsWhiteSpace(current) && !quoted)
            {
                if (argument.Length > 0)
                {
                    yield return argument.ToString();
                    argument.Clear();
                }
            }
            else
            {
                argument.Append(current);
            }
        }

        if (!quoted && argument.Length > 0) yield return argument.ToString();
    }
}
