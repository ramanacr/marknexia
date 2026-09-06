using System.Text.Json;
using Windows.ApplicationModel.DataTransfer;
using Windows.Storage.Pickers;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.Web.WebView2.Core;
using Marknexia.Core;
using Marknexia.Diagrams;
using Marknexia.Files;
using Marknexia.Infrastructure;
using Marknexia.Markdown;
using Marknexia.Navigation;
using Marknexia.Rendering;
using Marknexia.Security;
using Marknexia.Syntax;

namespace Marknexia.App;

public sealed partial class MainWindow : Window
{
    private readonly IPathCanonicalizer _canonicalizer;
    private readonly IFileService _fileService;
    private readonly IRepositoryService _repoService;
    private readonly INavigationResolver _navResolver;
    private readonly IMarkdownRenderer _renderer;
    private readonly INavigationHistoryManager _historyManager;
    private readonly SettingsService _settingsService;

    private readonly Dictionary<TabViewItem, DocumentTabState> _tabStates = new();
    private string? _activeRepositoryRoot;

    private static CoreWebView2Environment? _sharedWebViewEnvironment;
    private static readonly SemaphoreSlim _envLock = new(1, 1);

    private static async Task<CoreWebView2Environment> GetOrCreateWebViewEnvironmentAsync()
    {
        if (_sharedWebViewEnvironment != null)
        {
            return _sharedWebViewEnvironment;
        }

        await _envLock.WaitAsync();
        try
        {
            if (_sharedWebViewEnvironment == null)
            {
                string userDataFolder = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Marknexia", "WebView2Data");
                Directory.CreateDirectory(userDataFolder);
                _sharedWebViewEnvironment = await CoreWebView2Environment.CreateWithOptionsAsync(null, userDataFolder, null);
            }
            return _sharedWebViewEnvironment;
        }
        finally
        {
            _envLock.Release();
        }
    }

    public MainWindow()
    {
        _canonicalizer = new PathCanonicalizer();
        _fileService = new FileService(_canonicalizer);
        _repoService = new RepositoryService(_canonicalizer);
        _navResolver = new NavigationResolver(_canonicalizer, _fileService);
        _historyManager = new NavigationHistoryManager();
        _settingsService = new SettingsService();

        _renderer = new MarkdownRenderer(
            new MarkdigParserAdapter(),
            new HtmlSanitizerService(),
            new ColorCodeSyntaxHighlighter(),
            new DiagramRegistry(),
            new TemplateEngine());


        InitializeComponent();

        try
        {
            string cacheDir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Marknexia", "RenderCache");
            if (Directory.Exists(cacheDir))
            {
                Directory.Delete(cacheDir, true);
            }
        }
        catch { }

        ApplySavedTheme();
        RegisterKeyboardAccelerators();

        Closed += (sender, args) =>
        {
            try
            {
                string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
                string path = Path.Combine(localAppData, "Marknexia", "crash.log");
                File.AppendAllText(path, $"[{DateTime.Now:O}] [MainWindow_Closed]\n");
            }
            catch { }
        };

        // Process command-line argument after window loads
        if (Content is FrameworkElement rootElement)
        {
            rootElement.Loaded += (s, e) =>
            {
                try
                {
                    string[] cmdArgs = Environment.GetCommandLineArgs();
                    if (cmdArgs.Length > 1 && !string.IsNullOrWhiteSpace(cmdArgs[1]) && File.Exists(cmdArgs[1]))
                    {
                        _ = OpenDocumentInTabAsync(cmdArgs[1]);
                    }
                }
                catch (Exception ex)
                {
                    ShowDiagnostic($"Error loading startup document: {ex.Message}", DiagnosticSeverity.Warning);
                }
            };
        }
    }

    private void RegisterKeyboardAccelerators()
    {
        // Ctrl+O: Open File
        var openAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.O, Modifiers = Windows.System.VirtualKeyModifiers.Control };
        openAcc.Invoked += (s, e) => { OpenFile_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(openAcc);

        // Ctrl+Shift+O: Open Folder
        var folderAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.O, Modifiers = Windows.System.VirtualKeyModifiers.Control | Windows.System.VirtualKeyModifiers.Shift };
        folderAcc.Invoked += (s, e) => { OpenFolder_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(folderAcc);

        // Ctrl+F: Search
        var findAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.F, Modifiers = Windows.System.VirtualKeyModifiers.Control };
        findAcc.Invoked += (s, e) => { ToggleSearch_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(findAcc);

        // F5 / Ctrl+R: Reload
        var reloadAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.F5 };
        reloadAcc.Invoked += (s, e) => { Reload_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(reloadAcc);

        var reloadAcc2 = new KeyboardAccelerator { Key = Windows.System.VirtualKey.R, Modifiers = Windows.System.VirtualKeyModifiers.Control };
        reloadAcc2.Invoked += (s, e) => { Reload_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(reloadAcc2);

        // Alt+Left: Back
        var backAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.Left, Modifiers = Windows.System.VirtualKeyModifiers.Menu };
        backAcc.Invoked += (s, e) => { Back_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(backAcc);

        // Alt+Right: Forward
        var fwdAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.Right, Modifiers = Windows.System.VirtualKeyModifiers.Menu };
        fwdAcc.Invoked += (s, e) => { Forward_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(fwdAcc);

        // Ctrl+W: Close Tab
        var closeAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.W, Modifiers = Windows.System.VirtualKeyModifiers.Control };
        closeAcc.Invoked += (s, e) => { CloseCurrentTab(); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(closeAcc);

        // Esc: Close search
        var escAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.Escape };
        escAcc.Invoked += (s, e) => { CloseSearch_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(escAcc);
    }

    public async Task OpenDocumentInTabAsync(string filePath, string? targetAnchor = null)
    {
        string canonicalPath = _canonicalizer.CanonicalizePath(filePath);

        if (!_fileService.FileExists(canonicalPath))
        {
            ShowDiagnostic($"File not found: {canonicalPath}", DiagnosticSeverity.Error);
            return;
        }

        // Auto-detect repository root if none active
        if (string.IsNullOrEmpty(_activeRepositoryRoot))
        {
            _activeRepositoryRoot = _repoService.DetectRepositoryRoot(canonicalPath);
            if (!string.IsNullOrEmpty(_activeRepositoryRoot))
            {
                RefreshRepositoryFiles(_activeRepositoryRoot);
            }
        }

        // Check if tab already exists
        foreach (var (tabItem, state) in _tabStates)
        {
            if (state.FilePath.Equals(canonicalPath, StringComparison.OrdinalIgnoreCase))
            {
                DocumentTabView.SelectedItem = tabItem;
                if (!string.IsNullOrEmpty(targetAnchor))
                {
                    await state.WebView.ExecuteScriptAsync($"window.marknexiaBridge.scrollToAnchor('{targetAnchor}')");
                }
                return;
            }
        }

        // Read and Render
        FileReadResult readResult = await _fileService.ReadTextAsync(canonicalPath);
        AppTheme currentTheme = GetCurrentTheme();
        var renderContext = new RenderContext(canonicalPath, _activeRepositoryRoot, currentTheme);
        RenderedDocument rendered = await _renderer.RenderAsync(readResult.Content, renderContext);

        // Create Tab & WebView2
        var webView = new WebView2
        {
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            DefaultBackgroundColor = currentTheme == AppTheme.Dark
                ? Windows.UI.Color.FromArgb(255, 13, 17, 23)
                : Windows.UI.Color.FromArgb(255, 255, 255, 255)
        };

        var tabItemNew = new TabViewItem
        {
            Header = Path.GetFileName(canonicalPath),
            Content = webView,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            VerticalContentAlignment = VerticalAlignment.Stretch
        };

        var tabState = new DocumentTabState(canonicalPath, rendered, webView);
        _tabStates[tabItemNew] = tabState;
        DocumentTabView.TabItems.Add(tabItemNew);
        DocumentTabView.SelectedItem = tabItemNew;

        try
        {
            var env = await GetOrCreateWebViewEnvironmentAsync();
            await webView.EnsureCoreWebView2Async(env);
            webView.CoreWebView2.WebMessageReceived += CoreWebView2_WebMessageReceived;

            string? docDirectory = Path.GetDirectoryName(canonicalPath);
            if (!string.IsNullOrEmpty(docDirectory) && Directory.Exists(docDirectory))
            {
                webView.CoreWebView2.SetVirtualHostNameToFolderMapping(
                    "marknexia.viewer",
                    docDirectory,
                    CoreWebView2HostResourceAccessKind.Allow);
            }

            string assetsDirectory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Marknexia",
                "Assets");
            TemplateEngine.EnsureAssetsExtracted(assetsDirectory);

            webView.CoreWebView2.SetVirtualHostNameToFolderMapping(
                "marknexia.assets",
                assetsDirectory,
                CoreWebView2HostResourceAccessKind.Allow);

            if (rendered.HtmlContent.Length < 1_500_000)
            {
                try
                {
                    webView.NavigateToString(rendered.HtmlContent);
                }
                catch (ArgumentException)
                {
                    NavigateHtmlViaVirtualHost(webView, rendered.HtmlContent);
                }
            }
            else
            {
                NavigateHtmlViaVirtualHost(webView, rendered.HtmlContent);
            }
        }
        catch (Exception ex)
        {
            ShowDiagnostic($"WebView2 initialization failed:\n{ex}", DiagnosticSeverity.Error);
        }

        UpdateOutlineList(rendered.Headings);

        // Navigation history entry
        _historyManager.Push(new HistoryEntry(new DocumentUri(canonicalPath, targetAnchor), targetAnchor, 0, DateTimeOffset.UtcNow));
        UpdateButtonStates();

        if (!string.IsNullOrEmpty(targetAnchor))
        {
            _ = Task.Run(async () =>
            {
                await Task.Delay(400);
                DispatcherQueue.TryEnqueue(() =>
                {
                    _ = webView.ExecuteScriptAsync($"window.marknexiaBridge.scrollToAnchor('{targetAnchor}')");
                });
            });
        }
    }

    private void NavigateHtmlViaVirtualHost(WebView2 webView, string htmlContent)
    {
        string cacheDir = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Marknexia",
            "RenderCache");
        Directory.CreateDirectory(cacheDir);
        string fileId = $"{Guid.NewGuid():N}.html";
        string filePath = Path.Combine(cacheDir, fileId);
        File.WriteAllText(filePath, htmlContent, System.Text.Encoding.UTF8);

        webView.CoreWebView2.SetVirtualHostNameToFolderMapping(
            "marknexia.page",
            cacheDir,
            CoreWebView2HostResourceAccessKind.Allow);
        webView.CoreWebView2.Navigate($"https://marknexia.page/{fileId}");
    }

    private void CoreWebView2_WebMessageReceived(CoreWebView2 sender, CoreWebView2WebMessageReceivedEventArgs args)
    {
        try
        {
            using var doc = JsonDocument.Parse(args.WebMessageAsJson);
            var root = doc.RootElement;
            string type = root.GetProperty("type").GetString() ?? string.Empty;

            if (type == "openLink" && root.TryGetProperty("href", out var hrefProp))
            {
                string? href = hrefProp.GetString();
                HandleLinkClick(href);
            }
            else if (type == "copyText" && root.TryGetProperty("text", out var textProp))
            {
                string? text = textProp.GetString();
                if (!string.IsNullOrEmpty(text))
                {
                    var dataPackage = new DataPackage();
                    dataPackage.SetText(text);
                    Windows.ApplicationModel.DataTransfer.Clipboard.SetContent(dataPackage);
                }
            }
        }
        catch (Exception ex)
        {
            ShowDiagnostic($"Error handling message: {ex.Message}", DiagnosticSeverity.Warning);
        }
    }

    private async void HandleLinkClick(string? destination)
    {
        if (string.IsNullOrWhiteSpace(destination)) return;

        var activeTab = GetActiveTabState();
        string currentFile = activeTab?.FilePath ?? string.Empty;

        var context = new ResolutionContext(
            currentFile,
            _activeRepositoryRoot,
            null,
            new NavigationPolicy(_settingsService.Current.EnforceRepositorySandbox, _settingsService.Current.AllowExternalLinks, _settingsService.Current.AllowRemoteAssets));

        NavigationIntent intent = _navResolver.Resolve(destination, context);

        switch (intent.Kind)
        {
            case NavigationKind.SameDocumentAnchor:
                if (!string.IsNullOrEmpty(intent.Fragment) && activeTab != null)
                {
                    await activeTab.WebView.ExecuteScriptAsync($"window.marknexiaBridge.scrollToAnchor('{intent.Fragment}')");
                }
                break;

            case NavigationKind.CrossDocument:
            case NavigationKind.CrossDocumentWithAnchor:
            case NavigationKind.RepositoryRootRelative:
                if (intent.TargetDocument != null)
                {
                    await OpenDocumentInTabAsync(intent.TargetDocument.CanonicalPath, intent.Fragment);
                }
                break;

            case NavigationKind.ExternalBrowser:
                if (intent.ExternalUri != null)
                {
                    await Windows.System.Launcher.LaunchUriAsync(intent.ExternalUri);
                }
                break;

            case NavigationKind.BrokenTarget:
                ShowDiagnostic(intent.Diagnostic ?? "Broken link or missing target file.", DiagnosticSeverity.Warning);
                break;

            case NavigationKind.BlockedOrInvalid:
                ShowDiagnostic(intent.Diagnostic ?? "Blocked unsafe link destination.", DiagnosticSeverity.Error);
                break;
        }
    }

    private void UpdateOutlineList(IReadOnlyList<HeadingInfo> headings)
    {
        var items = headings.Select(h => new OutlineItemViewModel(h.Text, h.SlugId, h.Level)).ToList();
        OutlineListView.ItemsSource = items;
    }

    private void OutlineItem_Click(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is OutlineItemViewModel item)
        {
            var active = GetActiveTabState();
            if (active != null)
            {
                _ = active.WebView.ExecuteScriptAsync($"window.marknexiaBridge.scrollToAnchor('{item.SlugId}')");
            }
        }
    }

    private void RepositoryItem_Click(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is RepoFileViewModel file)
        {
            _ = OpenDocumentInTabAsync(file.FullPath);
        }
    }

    private void RefreshRepositoryFiles(string rootPath)
    {
        var files = _repoService.EnumerateWorkspaceFiles(rootPath);
        var viewModels = files.Select(f => new RepoFileViewModel(
            f,
            Path.GetRelativePath(rootPath, f),
            f.EndsWith(".md", StringComparison.OrdinalIgnoreCase) ? "\uE8A5" : "\uEB9F")).ToList();

        RepositoryListView.ItemsSource = viewModels;
    }

    private DocumentTabState? GetActiveTabState()
    {
        if (DocumentTabView?.SelectedItem is TabViewItem tab && _tabStates.TryGetValue(tab, out var state))
        {
            return state;
        }
        return null;
    }

    private void CloseCurrentTab()
    {
        if (DocumentTabView.SelectedItem is TabViewItem tab)
        {
            CloseTab(tab);
        }
    }

    private void CloseTab(TabViewItem tab)
    {
        _tabStates.Remove(tab);
        DocumentTabView.TabItems.Remove(tab);

        if (DocumentTabView.TabItems.Count > 0)
        {
            DocumentTabView.SelectedIndex = Math.Max(0, DocumentTabView.TabItems.Count - 1);
        }
        else
        {
            OutlineListView.ItemsSource = null;
        }
    }

    private void TabView_TabCloseRequested(TabView sender, TabViewTabCloseRequestedEventArgs args)
    {
        CloseTab(args.Tab);
    }

    private void TabView_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        var active = GetActiveTabState();
        if (active?.Document != null)
        {
            UpdateOutlineList(active.Document.Headings);
        }
        else
        {
            OutlineListView.ItemsSource = null;
        }
    }

    private void TabView_AddTabButtonClick(TabView sender, object args)
    {
        OpenFile_Click(this, new RoutedEventArgs());
    }

    private async void OpenFile_Click(object sender, RoutedEventArgs e)
    {
        var picker = new FileOpenPicker();
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        WinRT.Interop.InitializeWithWindow.Initialize(picker, hwnd);

        picker.ViewMode = PickerViewMode.List;
        picker.FileTypeFilter.Add(".md");
        picker.FileTypeFilter.Add(".markdown");
        picker.FileTypeFilter.Add(".mdown");
        picker.FileTypeFilter.Add(".mkdn");

        var file = await picker.PickSingleFileAsync();
        if (file != null)
        {
            await OpenDocumentInTabAsync(file.Path);
        }
    }

    private async void OpenFolder_Click(object sender, RoutedEventArgs e)
    {
        var picker = new FolderPicker();
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        WinRT.Interop.InitializeWithWindow.Initialize(picker, hwnd);

        picker.FileTypeFilter.Add("*");

        var folder = await picker.PickSingleFolderAsync();
        if (folder != null)
        {
            _activeRepositoryRoot = folder.Path;
            RefreshRepositoryFiles(_activeRepositoryRoot);
            SidebarModeSelector.SelectedIndex = 1; // Switch to Repository tab
            MainSplitView.IsPaneOpen = true;

            // If README.md exists, open it
            string readme = Path.Combine(_activeRepositoryRoot, "README.md");
            if (_fileService.FileExists(readme))
            {
                await OpenDocumentInTabAsync(readme);
            }
        }
    }

    private async void Reload_Click(object sender, RoutedEventArgs e)
    {
        var active = GetActiveTabState();
        if (active != null)
        {
            FileReadResult readResult = await _fileService.ReadTextAsync(active.FilePath);
            var renderContext = new RenderContext(active.FilePath, _activeRepositoryRoot, GetCurrentTheme());
            RenderedDocument rendered = await _renderer.RenderAsync(readResult.Content, renderContext);
            active.Document = rendered;
            AppTheme currentTheme = GetCurrentTheme();
            active.WebView.DefaultBackgroundColor = currentTheme == AppTheme.Dark
                ? Windows.UI.Color.FromArgb(255, 13, 17, 23)
                : Windows.UI.Color.FromArgb(255, 255, 255, 255);

            if (active.WebView.CoreWebView2 != null)
            {
                string? docDirectory = Path.GetDirectoryName(active.FilePath);
                if (!string.IsNullOrEmpty(docDirectory) && Directory.Exists(docDirectory))
                {
                    active.WebView.CoreWebView2.SetVirtualHostNameToFolderMapping(
                        "marknexia.viewer",
                        docDirectory,
                        CoreWebView2HostResourceAccessKind.Allow);
                }
            }

            active.WebView.NavigateToString(rendered.HtmlContent);
            UpdateOutlineList(rendered.Headings);
        }
    }

    private async void Back_Click(object sender, RoutedEventArgs e)
    {
        var active = GetActiveTabState();
        if (active != null && _historyManager.CanGoBack)
        {
            var currentEntry = new HistoryEntry(new DocumentUri(active.FilePath), null, 0, DateTimeOffset.UtcNow);
            var prev = _historyManager.GoBack(currentEntry);
            if (prev != null)
            {
                await OpenDocumentInTabAsync(prev.Document.CanonicalPath, prev.Fragment);
            }
            UpdateButtonStates();
        }
    }

    private async void Forward_Click(object sender, RoutedEventArgs e)
    {
        var active = GetActiveTabState();
        if (active != null && _historyManager.CanGoForward)
        {
            var currentEntry = new HistoryEntry(new DocumentUri(active.FilePath), null, 0, DateTimeOffset.UtcNow);
            var next = _historyManager.GoForward(currentEntry);
            if (next != null)
            {
                await OpenDocumentInTabAsync(next.Document.CanonicalPath, next.Fragment);
            }
            UpdateButtonStates();
        }
    }

    private void UpdateButtonStates()
    {
        BackButton.IsEnabled = _historyManager.CanGoBack;
        ForwardButton.IsEnabled = _historyManager.CanGoForward;
    }

    private void ToggleSidebar_Click(object sender, RoutedEventArgs e)
    {
        MainSplitView.IsPaneOpen = !MainSplitView.IsPaneOpen;
    }

    private void SidebarMode_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (SidebarModeSelector.SelectedIndex == 0)
        {
            OutlineListView.Visibility = Visibility.Visible;
            RepositoryListView.Visibility = Visibility.Collapsed;
        }
        else
        {
            OutlineListView.Visibility = Visibility.Collapsed;
            RepositoryListView.Visibility = Visibility.Visible;
        }
    }

    private void ToggleSearch_Click(object sender, RoutedEventArgs e)
    {
        SearchBarGrid.Visibility = SearchBarGrid.Visibility == Visibility.Visible ? Visibility.Collapsed : Visibility.Visible;
        if (SearchBarGrid.Visibility == Visibility.Visible)
        {
            SearchInputBox.Focus(FocusState.Programmatic);
        }
    }

    private void CloseSearch_Click(object sender, RoutedEventArgs e)
    {
        SearchBarGrid.Visibility = Visibility.Collapsed;
    }

    private async void FindNext_Click(object sender, RoutedEventArgs e)
    {
        string query = SearchInputBox.Text;
        if (string.IsNullOrWhiteSpace(query)) return;

        var active = GetActiveTabState();
        if (active != null)
        {
            await active.WebView.ExecuteScriptAsync($"window.find('{query}', false, false, true, false, false, false)");
        }
    }

    private async void FindPrevious_Click(object sender, RoutedEventArgs e)
    {
        string query = SearchInputBox.Text;
        if (string.IsNullOrWhiteSpace(query)) return;

        var active = GetActiveTabState();
        if (active != null)
        {
            await active.WebView.ExecuteScriptAsync($"window.find('{query}', false, true, true, false, false, false)");
        }
    }

    private void SearchBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == Windows.System.VirtualKey.Enter)
        {
            FindNext_Click(sender, new RoutedEventArgs());
            e.Handled = true;
        }
    }

    private void ThemeSelector_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_settingsService == null || ThemeSelector == null) return;

        if (ThemeSelector.SelectedItem is ComboBoxItem item && item.Tag is string tag)
        {
            AppTheme selectedTheme = tag switch
            {
                "Light" => AppTheme.Light,
                "Dark" => AppTheme.Dark,
                _ => AppTheme.System
            };

            _settingsService.Current.Theme = selectedTheme;
            _settingsService.Save(_settingsService.Current);
            ApplyThemeToRoot(selectedTheme);
            if (GetActiveTabState() != null)
            {
                Reload_Click(this, new RoutedEventArgs());
            }
        }
    }

    private void ApplySavedTheme()
    {
        ApplyThemeToRoot(_settingsService.Current.Theme);
    }

    private void ApplyThemeToRoot(AppTheme theme)
    {
        if (Content is FrameworkElement root)
        {
            root.RequestedTheme = theme switch
            {
                AppTheme.Light => ElementTheme.Light,
                AppTheme.Dark => ElementTheme.Dark,
                _ => ElementTheme.Default
            };
        }
    }

    private AppTheme GetCurrentTheme()
    {
        return _settingsService.Current.Theme;
    }

    private void ShowDiagnostic(string message, DiagnosticSeverity severity)
    {
        try
        {
            string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            string path = Path.Combine(localAppData, "Marknexia", "crash.log");
            File.AppendAllText(path, $"[{DateTime.Now:O}] [Diagnostic:{severity}] {message}\n");
        }
        catch { }

        DiagnosticInfoBar.Message = message;
        DiagnosticInfoBar.Severity = severity switch
        {
            DiagnosticSeverity.Error => InfoBarSeverity.Error,
            DiagnosticSeverity.Warning => InfoBarSeverity.Warning,
            _ => InfoBarSeverity.Informational
        };
        DiagnosticInfoBar.IsOpen = true;
    }
}

public sealed class DocumentTabState
{
    public string FilePath { get; set; }
    public RenderedDocument? Document { get; set; }
    public WebView2 WebView { get; set; }

    public DocumentTabState(string filePath, RenderedDocument? document, WebView2 webView)
    {
        FilePath = filePath;
        Document = document;
        WebView = webView;
    }
}

public sealed class OutlineItemViewModel
{
    public string Text { get; }
    public string SlugId { get; }
    public int Level { get; }
    public Thickness Margin => new Thickness((Level - 1) * 14, 2, 0, 2);

    public OutlineItemViewModel(string text, string slugId, int level)
    {
        Text = text;
        SlugId = slugId;
        Level = level;
    }
}

public sealed class RepoFileViewModel
{
    public string FullPath { get; }
    public string DisplayName { get; }
    public string IconGlyph { get; }

    public RepoFileViewModel(string fullPath, string displayName, string iconGlyph)
    {
        FullPath = fullPath;
        DisplayName = displayName;
        IconGlyph = iconGlyph;
    }
}

internal static class DispatcherQueueExtensions
{
    public static Task TryEnqueueAsync(this Microsoft.UI.Dispatching.DispatcherQueue dispatcher, Action action)
    {
        var tcs = new TaskCompletionSource();
        dispatcher.TryEnqueue(() =>
        {
            try
            {
                action();
                tcs.SetResult();
            }
            catch (Exception ex)
            {
                tcs.SetException(ex);
            }
        });
        return tcs.Task;
    }
}
