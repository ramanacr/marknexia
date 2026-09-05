using FluentAssertions;
using Marknexia.Core;
using Marknexia.Navigation;
using Xunit;

namespace Marknexia.Navigation.Tests;

public class NavigationHistoryManagerTests
{
    [Fact]
    public void HistoryManager_BackAndForward_MaintainsStateCorrectly()
    {
        var manager = new NavigationHistoryManager();
        var doc1 = new DocumentUri(@"C:\repo\README.md");
        var doc2 = new DocumentUri(@"C:\repo\docs\api.md");
        var doc3 = new DocumentUri(@"C:\repo\docs\nested.md");

        var entry1 = new HistoryEntry(doc1, null, 0, DateTimeOffset.UtcNow);
        var entry2 = new HistoryEntry(doc2, "auth", 100, DateTimeOffset.UtcNow);
        var entry3 = new HistoryEntry(doc3, null, 250, DateTimeOffset.UtcNow);

        manager.Push(entry1);
        manager.Push(entry2);

        manager.CanGoBack.Should().BeTrue();
        manager.CanGoForward.Should().BeFalse();

        // Go back from entry3 to entry2
        var prev = manager.GoBack(entry3);
        prev.Should().Be(entry2);
        manager.CanGoForward.Should().BeTrue();

        // Go forward back to entry3
        var next = manager.GoForward(entry2);
        next.Should().Be(entry3);
    }
}
