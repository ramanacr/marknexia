using FluentAssertions;
using Marknexia.Core;
using Xunit;

namespace Marknexia.Core.Tests;

public sealed class DocumentSearchStateTests
{
    [Fact]
    public void PendingResult_ForInactiveTabRemainsValidWhenAnotherTabSearches()
    {
        var first = new DocumentSearchState();
        var second = new DocumentSearchState();
        first.ApplyRenderedResult("read", 3, 1);
        long request = first.BeginRequest();
        second.SetQuery("other tab");
        second.BeginRequest();

        first.SetQuery("read"); // restoring its input must not cancel the request
        first.TryApplyRenderedResult(request, "read", 3, 2).Should().BeTrue();

        first.CurrentMatch.Should().Be(2);
        first.HasCurrentResult.Should().BeTrue();
        second.Query.Should().Be("other tab");
        second.HasCurrentResult.Should().BeFalse();
    }

    [Theory]
    [InlineData("query")]
    [InlineData("document")]
    [InlineData("newer-request")]
    public void StaleRequest_CannotOverwriteChangedSearchState(string invalidation)
    {
        var state = new DocumentSearchState();
        state.SetQuery("read");
        long request = state.BeginRequest();
        switch (invalidation)
        {
            case "query": state.SetQuery("different"); break;
            case "document": state.InvalidateResults(); break;
            case "newer-request": state.BeginRequest(); break;
        }

        state.TryApplyRenderedResult(request, "read", 3, 2).Should().BeFalse();
        state.HasCurrentResult.Should().BeFalse();
    }

    [Fact]
    public void CurrentRequest_RejectsMismatchedQueryAndAcceptsMatchingResultOnce()
    {
        var state = new DocumentSearchState();
        state.SetQuery("read");
        long request = state.BeginRequest();

        state.TryApplyRenderedResult(request, "other", 1, 1).Should().BeFalse();
        state.TryApplyRenderedResult(request, "read", 3, 2).Should().BeTrue();
        state.TryApplyRenderedResult(request, "read", 3, 3).Should().BeFalse();
        state.CurrentMatch.Should().Be(2);
    }

    [Fact]
    public void ApplyRenderedResult_AcceptsLiteralWhitespaceQuery()
    {
        var state = new DocumentSearchState();
        state.ApplyRenderedResult("  ", 1, 1);
        state.Query.Should().Be("  ");
        state.HasCurrentResult.Should().BeTrue();
        state.CurrentMatch.Should().Be(1);
    }

    [Fact]
    public void SetQuery_PreservesDraftAndInvalidatesOldResults()
    {
        var state = new DocumentSearchState();
        state.ApplyRenderedResult("read", 3, 2);

        state.SetQuery("new draft");

        state.Query.Should().Be("new draft");
        state.HasCurrentResult.Should().BeFalse();
        state.MatchCount.Should().Be(0);
        state.CurrentMatch.Should().Be(0);
    }

    [Fact]
    public void SetQuery_RestoringSameTabQueryKeepsItsPosition()
    {
        var state = new DocumentSearchState();
        state.ApplyRenderedResult("read", 3, 2);

        state.SetQuery("read");

        state.HasCurrentResult.Should().BeTrue();
        state.CurrentMatch.Should().Be(2);
    }

    [Fact]
    public void InvalidateResults_RetainsQueryWithoutClaimingNoMatches()
    {
        var state = new DocumentSearchState();
        state.ApplyRenderedResult("read", 3, 2);

        state.InvalidateResults();

        state.Query.Should().Be("read");
        state.HasCurrentResult.Should().BeFalse();
        state.MatchCount.Should().Be(0);
        state.CurrentMatch.Should().Be(0);
        state.ApplyRenderedResult("read", 0, 0);
        state.HasCurrentResult.Should().BeTrue();
    }

    [Fact]
    public void ApplyRenderedResult_UsesBrowserPositionInsteadOfRecountingMarkdown()
    {
        var state = new DocumentSearchState();

        state.ApplyRenderedResult("read", 3, 2);

        state.Query.Should().Be("read");
        state.MatchCount.Should().Be(3);
        state.CurrentMatch.Should().Be(2);
        state.ApplyRenderedResult("", 0, 0);
        state.CurrentMatch.Should().Be(0);
    }

    [Theory]
    [InlineData(-1, 0)]
    [InlineData(0, 1)]
    [InlineData(2, 0)]
    [InlineData(2, 3)]
    public void ApplyRenderedResult_RejectsInconsistentPositions(int count, int position)
    {
        var state = new DocumentSearchState();
        Action apply = () => state.ApplyRenderedResult("read", count, position);

        apply.Should().Throw<ArgumentOutOfRangeException>();
    }

    [Fact]
    public void Update_CountsMatchesCaseInsensitivelyAndWraps()
    {
        var state = new DocumentSearchState();

        state.Update("read", "Read, reread, READER");

        state.MatchCount.Should().Be(3);
        state.CurrentMatch.Should().Be(1);
        state.MoveNext();
        state.CurrentMatch.Should().Be(2);
        state.MovePrevious();
        state.CurrentMatch.Should().Be(1);
    }

    [Fact]
    public void Update_EmptyQueryClearsState()
    {
        var state = new DocumentSearchState();
        state.Update("x", "x");

        state.Update("", "x");

        state.MatchCount.Should().Be(0);
        state.CurrentMatch.Should().Be(0);
    }
}
