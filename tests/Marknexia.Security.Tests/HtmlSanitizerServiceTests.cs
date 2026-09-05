using FluentAssertions;
using Marknexia.Security;
using Xunit;

namespace Marknexia.Security.Tests;

public class HtmlSanitizerServiceTests
{
    private readonly HtmlSanitizerService _sanitizer = new();

    [Fact]
    public void SanitizeHtml_RemovesScriptTags()
    {
        string malicious = "<p>Hello</p><script>alert('xss');</script><span>World</span>";
        string clean = _sanitizer.SanitizeHtml(malicious);

        clean.Should().NotContain("<script");
        clean.Should().NotContain("alert");
        clean.Should().Contain("<p>Hello</p>");
        clean.Should().Contain("<span>World</span>");
    }

    [Fact]
    public void SanitizeHtml_StripsInlineEventHandlers()
    {
        string malicious = @"<img src=""x"" onerror=""alert(1)"" onclick=""steal()"" />";
        string clean = _sanitizer.SanitizeHtml(malicious);

        clean.Should().NotContain("onerror");
        clean.Should().NotContain("onclick");
        clean.Should().NotContain("alert");
    }

    [Fact]
    public void SanitizeHtml_StripsJavascriptUris()
    {
        string malicious = @"<a href=""javascript:alert(1)"">Click</a>";
        string clean = _sanitizer.SanitizeHtml(malicious);

        clean.Should().NotContain("javascript:");
    }

    [Fact]
    public void SanitizeSvg_StripsScriptsInsideSvg()
    {
        string maliciousSvg = @"<svg width=""100"" height=""100""><circle cx=""50"" cy=""50"" r=""40"" /><script>alert(1)</script></svg>";
        string clean = _sanitizer.SanitizeSvg(maliciousSvg);

        clean.Should().NotContain("<script");
        clean.Should().NotContain("alert");
        clean.Should().Contain("<circle");
    }

    [Fact]
    public void SanitizeHtml_PreservesCustomAnchors()
    {
        string anchor = @"<a name=""my-custom-anchor""></a>";
        string clean = _sanitizer.SanitizeHtml(anchor);

        clean.Should().Contain("name=\"my-custom-anchor\"");
    }
}
