using FluentAssertions;
using Marknexia.Files;
using Xunit;

namespace Marknexia.Files.Tests;

public sealed class RepositoryTreeBuilderTests
{
    [Theory]
    [InlineData("artifacts")]
    [InlineData("ARTIFACTS")]
    [InlineData("dist")]
    [InlineData("publish")]
    [InlineData("TestResults")]
    [InlineData("coverage")]
    [InlineData(".codegraph")]
    public void Build_DoesNotExposeGeneratedOutputAsRepositoryContent(string generatedFolder)
    {
        string root = Path.Combine(Path.GetTempPath(), "marknexia-output-tree-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(Path.Combine(root, generatedFolder, "nested"));
        Directory.CreateDirectory(Path.Combine(root, "docs", "artifact-guide"));
        File.WriteAllText(Path.Combine(root, generatedFolder, "nested", "generated.md"), "# Generated output");
        File.WriteAllText(Path.Combine(root, "docs", "artifact-guide", "README.md"), "# Authored guide");
        try
        {
            var tree = new RepositoryTreeBuilder(new PathCanonicalizer()).Build(root);

            tree.Children.Select(node => node.DisplayName).Should().Equal("docs");
            tree.Children[0].Children.Should().ContainSingle().Which.DisplayName.Should().Be("artifact-guide");
        }
        finally
        {
            Directory.Delete(root, recursive: true);
        }
    }

    [Fact]
    public void Build_ProducesFoldersBeforeFilesAndIgnoresBuildDirectories()
    {
        string root = Path.Combine(Path.GetTempPath(), "marknexia-tree-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(Path.Combine(root, "docs", "nested"));
        Directory.CreateDirectory(Path.Combine(root, "bin"));
        File.WriteAllText(Path.Combine(root, "README.md"), "# Read");
        File.WriteAllText(Path.Combine(root, "docs", "nested", "guide.md"), "# Guide");
        File.WriteAllText(Path.Combine(root, "bin", "ignored.md"), "# Ignore");

        try
        {
            var tree = new RepositoryTreeBuilder(new PathCanonicalizer()).Build(root);

            tree.Children.Select(x => x.DisplayName).Should().ContainInOrder("docs", "README.md");
            tree.Children.Should().NotContain(x => x.DisplayName == "bin");
            tree.Children[0].Children.Single().DisplayName.Should().Be("nested");
        }
        finally
        {
            if (Directory.Exists(root)) Directory.Delete(root, true);
        }
    }

    [Fact]
    public async Task BuildAsync_ReturnsTheSameSandboxedTreeWithoutChangingTheSynchronousContract()
    {
        string root = Path.Combine(Path.GetTempPath(), "marknexia-async-tree-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(Path.Combine(root, "docs"));
        File.WriteAllText(Path.Combine(root, "docs", "README.md"), "# Read");

        try
        {
            var tree = await new RepositoryTreeBuilder(new PathCanonicalizer()).BuildAsync(root);

            tree.Children.Should().ContainSingle().Which.DisplayName.Should().Be("docs");
            tree.Children[0].Children.Should().ContainSingle().Which.DisplayName.Should().Be("README.md");
        }
        finally
        {
            if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        }
    }
}
