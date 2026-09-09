namespace Marknexia.Core;

public sealed class DocumentSearchState
{
    private long _requestRevision;
    public string Query { get; private set; } = string.Empty;
    public int MatchCount { get; private set; }
    public int CurrentMatch { get; private set; }
    public bool HasCurrentResult { get; private set; }

    public long BeginRequest()
    {
        HasCurrentResult = false;
        return ++_requestRevision;
    }

    public bool IsCurrentRequest(long request) => request > 0 && request == _requestRevision;

    public bool TryApplyRenderedResult(long request, string query, int matchCount, int currentMatch)
    {
        if (!IsCurrentRequest(request) || !string.Equals(Query, query, StringComparison.Ordinal)) return false;
        ApplyRenderedResult(query, matchCount, currentMatch);
        return true;
    }

    public void SetQuery(string query)
    {
        ArgumentNullException.ThrowIfNull(query);
        if (Query == query) return;
        Query = query;
        InvalidateResults();
    }

    public void InvalidateResults()
    {
        ++_requestRevision;
        HasCurrentResult = false;
        MatchCount = 0;
        CurrentMatch = 0;
    }

    public void ApplyRenderedResult(string query, int matchCount, int currentMatch)
    {
        ArgumentNullException.ThrowIfNull(query);
        if (matchCount < 0 || currentMatch < 0 || currentMatch > matchCount
            || (matchCount > 0 && currentMatch == 0) || (query.Length == 0 && matchCount != 0))
            throw new ArgumentOutOfRangeException(nameof(currentMatch), "Rendered match position must agree with its count.");

        Query = query;
        MatchCount = matchCount;
        CurrentMatch = currentMatch;
        HasCurrentResult = true;
        ++_requestRevision;
    }

    public void Update(string? query, string? source)
    {
        ++_requestRevision;
        Query = query ?? string.Empty;
        HasCurrentResult = true;
        if (string.IsNullOrEmpty(Query) || string.IsNullOrEmpty(source))
        {
            MatchCount = 0;
            CurrentMatch = 0;
            return;
        }

        MatchCount = CountMatches(source, Query);
        CurrentMatch = MatchCount == 0 ? 0 : 1;
    }

    public void MoveNext() => CurrentMatch = MatchCount == 0 ? 0 : CurrentMatch == MatchCount ? 1 : CurrentMatch + 1;
    public void MovePrevious() => CurrentMatch = MatchCount == 0 ? 0 : CurrentMatch <= 1 ? MatchCount : CurrentMatch - 1;

    private static int CountMatches(string source, string query)
    {
        int count = 0;
        int index = 0;
        while ((index = source.IndexOf(query, index, StringComparison.OrdinalIgnoreCase)) >= 0)
        {
            count++;
            index += query.Length;
        }
        return count;
    }
}
