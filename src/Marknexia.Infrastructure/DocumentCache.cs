using System.Collections.Concurrent;
using Marknexia.Core;

namespace Marknexia.Infrastructure;

public sealed class DocumentCache
{
    private readonly ConcurrentDictionary<string, CacheEntry<FileReadResult>> _sourceCache = new(StringComparer.OrdinalIgnoreCase);
    private readonly ConcurrentDictionary<string, CacheEntry<RenderedDocument>> _renderCache = new(StringComparer.Ordinal);

    private const int MaxCacheEntries = 100;

    public bool TryGetSource(string canonicalPath, DateTime lastWriteUtc, out FileReadResult? result)
    {
        if (_sourceCache.TryGetValue(canonicalPath, out var entry) && entry.LastWriteUtc == lastWriteUtc)
        {
            result = entry.Value;
            return true;
        }

        result = null;
        return false;
    }

    public void SetSource(string canonicalPath, DateTime lastWriteUtc, FileReadResult result)
    {
        if (_sourceCache.Count > MaxCacheEntries)
        {
            _sourceCache.Clear();
        }

        _sourceCache[canonicalPath] = new CacheEntry<FileReadResult>(result, lastWriteUtc);
    }

    public bool TryGetRendered(string contentHash, AppTheme theme, out RenderedDocument? document)
    {
        string key = $"{contentHash}:{theme}";
        if (_renderCache.TryGetValue(key, out var entry))
        {
            document = entry.Value;
            return true;
        }

        document = null;
        return false;
    }

    public void SetRendered(string contentHash, AppTheme theme, RenderedDocument document)
    {
        if (_renderCache.Count > MaxCacheEntries)
        {
            _renderCache.Clear();
        }

        string key = $"{contentHash}:{theme}";
        _renderCache[key] = new CacheEntry<RenderedDocument>(document, DateTime.UtcNow);
    }

    public void Invalidate(string canonicalPath)
    {
        _sourceCache.TryRemove(canonicalPath, out _);
    }

    public void Clear()
    {
        _sourceCache.Clear();
        _renderCache.Clear();
    }

    private sealed record CacheEntry<T>(T Value, DateTime LastWriteUtc);
}
