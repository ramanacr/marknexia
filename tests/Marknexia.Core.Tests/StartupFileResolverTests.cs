using FluentAssertions;
using Marknexia.Core;
using Xunit;

namespace Marknexia.Core.Tests;

public sealed class StartupFileResolverTests
{
    [Theory]
    [InlineData("--open \"{0}\"")]
    [InlineData("\"C:\\Program Files\\Marknexia\\Marknexia.App.exe\" \"{0}\"")]
    [InlineData("\"{0}\" \"missing second file.md\"")]
    public void FindFirstExisting_Extracts_document_from_activation_command_line(string commandLine)
    {
        string existing = Path.Combine(Path.GetTempPath(), $"marknexia activation {Guid.NewGuid():N}.md");
        File.WriteAllText(existing, "# Activation");
        try
        {
            StartupFileResolver.FindFirstExisting([string.Format(commandLine, existing)]).Should().Be(existing);
        }
        finally { File.Delete(existing); }
    }

    [Fact]
    public void FindFirstExisting_Accepts_file_uri_and_skips_existing_executable()
    {
        string existing = Path.Combine(Path.GetTempPath(), $"marknexia #activation {Guid.NewGuid():N}.md");
        File.WriteAllText(existing, "# Activation");
        try
        {
            StartupFileResolver.FindFirstExisting([Environment.ProcessPath, new Uri(existing).AbsoluteUri]).Should().Be(existing);
        }
        finally { File.Delete(existing); }
    }

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
