using FluentAssertions;
using Marknexia.Files;
using Xunit;

namespace Marknexia.Files.Tests;

public class PathCanonicalizerTests
{
    private readonly PathCanonicalizer _canonicalizer = new();

    [Fact]
    public void CanonicalizePath_NormalizesSlashesAndDotDots()
    {
        string input = @"C:\folder\sub/../file.md";
        string result = _canonicalizer.CanonicalizePath(input);

        result.Should().Be(@"C:\folder\file.md");
    }

    [Fact]
    public void IsWithinRoot_DetectsEscapes()
    {
        string root = @"C:\repo";
        string safe = @"C:\repo\docs\api.md";
        string escape = @"C:\other\secret.md";
        string prefixMismatch = @"C:\repo-evil\secret.md";

        _canonicalizer.IsWithinRoot(safe, root).Should().BeTrue();
        _canonicalizer.IsWithinRoot(escape, root).Should().BeFalse();
        _canonicalizer.IsWithinRoot(prefixMismatch, root).Should().BeFalse();
    }
}
