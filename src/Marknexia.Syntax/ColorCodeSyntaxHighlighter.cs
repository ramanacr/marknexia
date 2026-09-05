using System.Net;
using ColorCode;
using ColorCode.Common;
using Marknexia.Core;

namespace Marknexia.Syntax;

public sealed class ColorCodeSyntaxHighlighter : ISyntaxHighlighter
{
    private readonly HtmlClassFormatter _formatter = new();

    public string HighlightCode(string code, string? language)
    {
        if (string.IsNullOrEmpty(code))
        {
            return string.Empty;
        }

        ILanguage? langObj = ResolveLanguage(language);
        if (langObj == null)
        {
            // Plain text fallback
            return $"<pre><code>{WebUtility.HtmlEncode(code)}</code></pre>";
        }

        try
        {
            return _formatter.GetHtmlString(code, langObj);
        }
        catch
        {
            // Advisory highlighting: never throw on parser/tokenization failure
            return $"<pre><code>{WebUtility.HtmlEncode(code)}</code></pre>";
        }
    }

    private static ILanguage? ResolveLanguage(string? language)
    {
        if (string.IsNullOrWhiteSpace(language)) return null;

        string normalized = language.Trim().ToLowerInvariant();
        return normalized switch
        {
            "c#" or "cs" => Languages.FindById("csharp") ?? Languages.CSharp,
            "c++" or "c" => Languages.FindById("cpp") ?? Languages.Cpp,
            "js" => Languages.FindById("javascript") ?? Languages.JavaScript,
            "ts" => Languages.FindById("typescript") ?? Languages.FindById("javascript"),
            "md" => Languages.FindById("markdown") ?? Languages.Markdown,
            "ps1" or "posh" => Languages.FindById("powershell") ?? Languages.PowerShell,
            _ => Languages.FindById(normalized)
        };
    }
}
