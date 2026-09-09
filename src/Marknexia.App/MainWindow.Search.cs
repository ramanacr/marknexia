using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Web.WebView2.Core;
using Marknexia.Core;

namespace Marknexia.App;

public sealed partial class MainWindow
{
    private void InvalidateSearchForNavigation(WebView2 webView)
    {
        var state = _tabStates.FirstOrDefault(tab => tab.WebView == webView);
        if (state is null) return;
        state.IsDocumentReady = false;
        state.Search.InvalidateResults();
        if (GetActiveTabState() == state) RefreshSearchForActiveTab();
    }

    private void AttachSearchNavigationHandlers(WebView2 webView, CoreWebView2 core)
    {
        core.NavigationStarting += (_, args) =>
        {
            var state = _tabStates.FirstOrDefault(tab => tab.WebView == webView);
            if (state is null) return;
            state.NavigationId = args.NavigationId;
            InvalidateSearchForNavigation(webView);
        };
        core.NavigationCompleted += async (_, args) =>
        {
            var state = _tabStates.FirstOrDefault(tab => tab.WebView == webView);
            if (state is null || state.NavigationId != args.NavigationId) return;
            state.IsDocumentReady = args.IsSuccess;
            if (state.IsDocumentReady)
                await ApplyPendingLocationAsync(state);
            if (GetActiveTabState() != state) return;
            RefreshSearchForActiveTab();
            if (state.IsDocumentReady && SearchBarGrid.Visibility == Visibility.Visible
                && !string.IsNullOrEmpty(state.Search.Query))
                await FindInRenderedDocumentAsync(false);
        };
    }

    private void RefreshSearchForActiveTab()
    {
        if (SearchInputBox is null || SearchCountText is null) return;
        var search = GetActiveTabState()?.Search;
        SearchInputBox.Text = search?.Query ?? string.Empty;
        SearchCountText.Text = search is null || !search.HasCurrentResult || string.IsNullOrEmpty(search.Query) ? string.Empty
            : search.MatchCount == 0 ? "No matches" : $"{search.CurrentMatch} of {search.MatchCount}";
    }

    private void SearchInputBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        if (SearchInputBox is null || DocumentTabsView is null) return;
        GetActiveTabState()?.Search.SetQuery(SearchInputBox.Text);
        if (SearchCountText is not null) SearchCountText.Text = string.Empty;
    }

    private void ToggleSearch_Click(object sender, RoutedEventArgs e)
    {
        if (SearchBarGrid.Visibility == Visibility.Visible) { CloseSearch_Click(sender, e); return; }
        SearchBarGrid.Visibility = Visibility.Visible;
        RefreshSearchForActiveTab();
        SearchInputBox.Focus(FocusState.Programmatic);
    }

    private async void CloseSearch_Click(object sender, RoutedEventArgs e)
    {
        SearchBarGrid.Visibility = Visibility.Collapsed;
        SearchInputBox.Text = string.Empty;
        SearchCountText.Text = string.Empty;
        var active = GetActiveTabState();
        if (active is null) return;
        active.Search.ApplyRenderedResult(string.Empty, 0, 0);
        try { await active.WebView.ExecuteScriptAsync("window.marknexiaBridge?.clearSearch()"); }
        catch (Exception ex) when (ex is InvalidOperationException or System.Runtime.InteropServices.COMException) { }
    }

    private async void FindNext_Click(object sender, RoutedEventArgs e) => await FindInRenderedDocumentAsync(false);
    private async void FindPrevious_Click(object sender, RoutedEventArgs e) => await FindInRenderedDocumentAsync(true);

    private async Task FindInRenderedDocumentAsync(bool backwards)
    {
        var active = GetActiveTabState();
        if (active is null) return;
        string query = SearchInputBox.Text;
        active.Search.SetQuery(query);
        long request = active.Search.BeginRequest();
        if (!active.IsDocumentReady)
        {
            SearchCountText.Text = "Document loading…";
            return;
        }
        try
        {
            string json = await active.WebView.ExecuteScriptAsync(
                $"window.marknexiaBridge?.findText({JsArg(query)}, {(backwards ? "true" : "false")})");
            if (!_tabStates.Contains(active) || !active.Search.IsCurrentRequest(request)) return;
            var result = JsonSerializer.Deserialize<RenderedSearchResult>(json);
            if (result is null)
            {
                if (GetActiveTabState() == active) SearchCountText.Text = "Document loading…";
                return;
            }
            if (!active.Search.TryApplyRenderedResult(request, result.Query, result.MatchCount, result.CurrentMatch)) return;
            // A tab switch changes the visible controls, not ownership of this
            // response. Save valid inactive-tab results without touching the UI.
            if (GetActiveTabState() != active || SearchBarGrid.Visibility != Visibility.Visible) return;
            SearchCountText.Text = query.Length == 0 ? string.Empty : result.MatchCount == 0
                ? "No matches" : $"{result.CurrentMatch} of {result.MatchCount}";
        }
        catch (Exception ex) when (ex is JsonException or ArgumentException or InvalidOperationException or System.Runtime.InteropServices.COMException)
        {
            if (!active.Search.IsCurrentRequest(request) || GetActiveTabState() != active) return;
            SearchCountText.Text = string.Empty;
            ShowDiagnostic("Find is temporarily unavailable. Wait for the document to load and try again.", DiagnosticSeverity.Warning);
        }
    }

    private sealed record RenderedSearchResult(
        [property: JsonPropertyName("query")] string Query,
        [property: JsonPropertyName("matchCount")] int MatchCount,
        [property: JsonPropertyName("currentMatch")] int CurrentMatch);
}
