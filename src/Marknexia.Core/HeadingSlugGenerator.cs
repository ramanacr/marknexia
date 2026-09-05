using System.Text;
using System.Text.RegularExpressions;

namespace Marknexia.Core;

public sealed class HeadingSlugGenerator
{
    private readonly Dictionary<string, int> _slugCounts = new(StringComparer.OrdinalIgnoreCase);

    public string GenerateSlug(string headingText)
    {
        if (string.IsNullOrWhiteSpace(headingText))
        {
            return string.Empty;
        }

        // 1. Strip HTML tags
        string clean = Regex.Replace(headingText, @"<[^>]*>", string.Empty);

        // 2. Strip Markdown formatting characters: bold, italic, code ticks, strikethrough
        clean = Regex.Replace(clean, @"[\*_`~]", string.Empty);

        // 3. Trim and lowercase invariant
        clean = clean.Trim().ToLowerInvariant();

        // 4. Filter characters: retain letters, digits, '-', '_', and replace whitespace with '-'
        var sb = new StringBuilder();
        foreach (char c in clean)
        {
            if (char.IsLetterOrDigit(c) || c == '-' || c == '_')
            {
                sb.Append(c);
            }
            else if (char.IsWhiteSpace(c))
            {
                sb.Append('-');
            }
        }

        // 5. Collapse consecutive hyphens and trim
        string baseSlug = Regex.Replace(sb.ToString(), @"-+", "-").Trim('-');
        if (string.IsNullOrEmpty(baseSlug))
        {
            baseSlug = "heading";
        }

        // 6. De-duplicate deterministically
        if (_slugCounts.TryGetValue(baseSlug, out int count))
        {
            count++;
            _slugCounts[baseSlug] = count;
            return $"{baseSlug}-{count}";
        }
        else
        {
            _slugCounts[baseSlug] = 0;
            return baseSlug;
        }
    }

    public void Reset()
    {
        _slugCounts.Clear();
    }
}
