using Marknexia.Core;

namespace Marknexia.Navigation;

public sealed class NavigationHistoryManager : INavigationHistoryManager
{
    private readonly Stack<HistoryEntry> _backStack = new();
    private readonly Stack<HistoryEntry> _forwardStack = new();

    public bool CanGoBack => _backStack.Count > 0;
    public bool CanGoForward => _forwardStack.Count > 0;

    public IReadOnlyList<HistoryEntry> BackStack => _backStack.ToArray();
    public IReadOnlyList<HistoryEntry> ForwardStack => _forwardStack.ToArray();

    public void Push(HistoryEntry entry)
    {
        if (entry == null) return;
        _backStack.Push(entry);
        _forwardStack.Clear();
    }

    public HistoryEntry? GoBack(HistoryEntry currentEntry)
    {
        if (!CanGoBack) return null;

        _forwardStack.Push(currentEntry);
        return _backStack.Pop();
    }

    public HistoryEntry? GoForward(HistoryEntry currentEntry)
    {
        if (!CanGoForward) return null;

        _backStack.Push(currentEntry);
        return _forwardStack.Pop();
    }

    public void Clear()
    {
        _backStack.Clear();
        _forwardStack.Clear();
    }
}
