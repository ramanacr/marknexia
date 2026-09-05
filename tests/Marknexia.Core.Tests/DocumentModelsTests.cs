using FluentAssertions;
using Marknexia.Core;
using Xunit;

namespace Marknexia.Core.Tests;

public class DocumentModelsTests
{
    [Fact]
    public void DocumentUri_ToString_WithFragment_ReturnsExpectedFormat()
    {
        var uri = new DocumentUri(@"C:\repo\README.md", "overview");
        uri.ToString().Should().Be(@"C:\repo\README.md#overview");
    }

    [Fact]
    public void DocumentUri_ToString_WithoutFragment_ReturnsPathOnly()
    {
        var uri = new DocumentUri(@"C:\repo\README.md");
        uri.ToString().Should().Be(@"C:\repo\README.md");
    }

    [Fact]
    public void NavigationIntent_HoldsPropertiesCorrectly()
    {
        var targetDoc = new DocumentUri(@"C:\repo\docs\api.md", "auth");
        var intent = new NavigationIntent(NavigationKind.CrossDocumentWithAnchor, targetDoc, "auth", null, true);

        intent.Kind.Should().Be(NavigationKind.CrossDocumentWithAnchor);
        intent.TargetDoc().Should().Be(targetDoc);
        intent.Fragment.Should().Be("auth");
        intent.IsSafe.Should().BeTrue();
    }
}

internal static class TestExtensions
{
    public static DocumentUri? TargetDoc(this NavigationIntent intent) => intent.TargetDocument;
}
