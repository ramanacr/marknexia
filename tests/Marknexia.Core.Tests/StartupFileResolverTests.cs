using FluentAssertions;
using Marknexia.Core;
using Xunit;

namespace Marknexia.Core.Tests;

public sealed class StartupFileResolverTests
{
    [Fact]
    public void FindFirstExisting_ReturnsTheFirstValidCandidateInActivationOrder()
    {
        string existing = Path.Combine(Path.GetTempPath(), $"marknexia-startup-{Guid.NewGuid():N}.md");
        File.WriteAllText(existing, "# Startup");

        try
        {
            string? result = StartupFileResolver.FindFirstExisting(
                [null, "", "missing.md", existing]);

            result.Should().Be(Path.GetFullPath(existing));
        }
        finally
        {
            if (File.Exists(existing)) File.Delete(existing);
        }
    }

    [Fact]
    public void FindFirstExisting_ReturnsNullWhenNoCandidateIsAFile()
    {
        StartupFileResolver.FindFirstExisting([null, "", "missing.md"]).Should().BeNull();
    }

    [Fact]
    public void FindFirstExisting_AcceptsQuotedPathsFromCommandLineActivation()
    {
        string existing = Path.Combine(Path.GetTempPath(), $"marknexia startup {Guid.NewGuid():N}.md");
        File.WriteAllText(existing, "# Startup");

        try
        {
            StartupFileResolver.FindFirstExisting([$"\"{existing}\""]).Should().Be(Path.GetFullPath(existing));
        }
        finally
        {
            if (File.Exists(existing)) File.Delete(existing);
        }
    }
}
