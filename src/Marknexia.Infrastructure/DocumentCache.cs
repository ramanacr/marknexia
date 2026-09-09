using System.Collections.Concurrent;
using Marknexia.Core;

namespace Marknexia.Infrastructure;

public sealed record DocumentRenderCacheKey(
    string ContentHash,
    string SourcePath,
    string? RepositoryRoot,
    AppTheme Theme,
    bool EnableDiagrams,
    bool EnableMath,
    bool AllowRemoteAssets,
    string RendererConfigurationVersion);

public sealed class DocumentCache
{
    private readonly ConcurrentDictionary<string, CacheEntry<FileReadResult>> _sourceCache = new(StringComparer.OrdinalIgnoreCase);
    private readonly ConcurrentDictionary<DocumentRenderCacheKey, CacheEntry<RenderedDocument>> _renderCache = new();

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

    public bool TryGetSource(string canonicalPath, DateTime lastWriteUtc, long fileSize, out FileReadResult? result)
    {
        if (_sourceCache.TryGetValue(canonicalPath, out var entry)
            && entry.LastWriteUtc == lastWriteUtc
            && entry.Value.SizeBytes == fileSize)
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

    public bool TryGetRendered(DocumentRenderCacheKey key, out RenderedDocument? document)
    {
        if (_renderCache.TryGetValue(key, out var entry))
        {
            document = entry.Value;
            return true;
        }

        document = null;
        return false;
    }

    public void SetRendered(DocumentRenderCacheKey key, RenderedDocument document)
    {
        if (_renderCache.Count > MaxCacheEntries)
        {
            _renderCache.Clear();
        }

        _renderCache[key] = new CacheEntry<RenderedDocument>(document, DateTime.UtcNow);
    }

    // Preserve the original small API for consumers that do not render local
    // assets. New callers should use the context-aware key overload above.
    public bool TryGetRendered(string contentHash, AppTheme theme, out RenderedDocument? document) =>
        TryGetRendered(new DocumentRenderCacheKey(contentHash, string.Empty, null, theme, true, true, false, "legacy"), out document);

    public void SetRendered(string contentHash, AppTheme theme, RenderedDocument document) =>
        SetRendered(new DocumentRenderCacheKey(contentHash, string.Empty, null, theme, true, true, false, "legacy"), document);

    public void Invalidate(string canonicalPath)
    {
        _sourceCache.TryRemove(canonicalPath, out _);
        foreach (DocumentRenderCacheKey key in _renderCache.Keys)
        {
            if (key.SourcePath.Equals(canonicalPath, StringComparison.OrdinalIgnoreCase))
            {
                _renderCache.TryRemove(key, out _);
            }
        }
    }

    public void Clear()
    {
        _sourceCache.Clear();
        _renderCache.Clear();
    }

    private sealed record CacheEntry<T>(T Value, DateTime LastWriteUtc);
}
