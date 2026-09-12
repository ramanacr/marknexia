using System.Text.Json;
using System.Diagnostics;
using System.Reflection;
using Windows.ApplicationModel.DataTransfer;
using Windows.Foundation;
using Windows.Foundation.Collections;
using Windows.Storage;
using Windows.Storage.Pickers;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Windowing;
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
    private const string RendererConfigurationVersion = "2026-09-12-render-v4";
    private const double MinimumSidebarWidth = 220;
    private const double MaximumSidebarWidth = 520;
    private readonly string? _startupFilePath;
    private readonly IPathCanonicalizer _canonicalizer;
    private readonly IFileService _fileService;
    private readonly IRepositoryService _repoService;
    private readonly INavigationResolver _navResolver;
    private readonly IMarkdownRenderer _renderer;
    private readonly SettingsService _settingsService;
    private readonly UpdateService _updateService;
    private readonly RepositoryTreeBuilder _treeBuilder;
    private readonly DocumentCache _documentCache = new();

    private readonly List<DocumentTabState> _tabStates = new();
    private CancellationTokenSource _repositoryRefreshCancellation = new();
    private string? _activeRepositoryRoot;
    private bool _isInitializing = true;
    private bool _hasProcessedStartup;
    private bool _isSynchronizingTabs;
    private bool _isResizingSidebar;

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

    public MainWindow(string? startupFilePath = null)
    {
        _startupFilePath = startupFilePath;
        _canonicalizer = new PathCanonicalizer();
        _fileService = new FileService(_canonicalizer);
        _repoService = new RepositoryService(_canonicalizer);
        _navResolver = new NavigationResolver(_canonicalizer, _fileService);
        _settingsService = new SettingsService();
        _updateService = new UpdateService();
        _treeBuilder = new RepositoryTreeBuilder(_canonicalizer);

        _renderer = new MarkdownRenderer(
            new MarkdigParserAdapter(),
            new HtmlSanitizerService(),
            new ColorCodeSyntaxHighlighter(),
            new DiagramRegistry(),
            new TemplateEngine());


        InitializeComponent();

        ConfigureWindowIcon();
        ApplySavedTheme();
        MainSplitView.IsPaneOpen = _settingsService.Current.IsSidebarOpen;
        MainSplitView.OpenPaneLength = Math.Clamp(_settingsService.Current.SidebarWidth, MinimumSidebarWidth, MaximumSidebarWidth);
        SidebarModeSelector.SelectedIndex = Math.Clamp(_settingsService.Current.SidebarMode, 0, 1);
        RegisterKeyboardAccelerators();
        RefreshRecentFiles();
        RemoteAssetsMenuItem.IsChecked = _settingsService.Current.AllowRemoteAssets;
        _isInitializing = false;
        ApplySidebarModeVisuals();
        UpdateStartPageAffordance();

        Closed += (sender, args) =>
        {
            try
            {
                _repositoryRefreshCancellation.Cancel();
                _repositoryRefreshCancellation.Dispose();
                string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
                string path = Path.Combine(localAppData, "Marknexia", "crash.log");
                File.AppendAllText(path, $"[{DateTime.Now:O}] [MainWindow_Closed]\n");
            }
            catch { }
        };

        // Restore the repository tree and process activation after the shell is
        // loaded. Filesystem work stays off the WinUI thread until its result
        // is ready to attach to the native tree control.
        if (Content is FrameworkElement rootElement)
        {
            if (rootElement.IsLoaded)
            {
                _ = InitializeAfterLoadAsync();
            }
            else
            {
                rootElement.Loaded += (s, e) =>
                {
                    _ = InitializeAfterLoadAsync();
                };
            }
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

        var newTabAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.T, Modifiers = Windows.System.VirtualKeyModifiers.Control };
        newTabAcc.Invoked += (s, e) => { OpenFile_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(newTabAcc);

        var nextTabAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.Tab, Modifiers = Windows.System.VirtualKeyModifiers.Control };
        nextTabAcc.Invoked += (s, e) => { SelectAdjacentTab(false); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(nextTabAcc);

        var previousTabAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.Tab, Modifiers = Windows.System.VirtualKeyModifiers.Control | Windows.System.VirtualKeyModifiers.Shift };
        previousTabAcc.Invoked += (s, e) => { SelectAdjacentTab(true); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(previousTabAcc);

        // Esc: Close search
        var escAcc = new KeyboardAccelerator { Key = Windows.System.VirtualKey.Escape };
        escAcc.Invoked += (s, e) => { CloseSearch_Click(this, new RoutedEventArgs()); e.Handled = true; };
        (Content as FrameworkElement)?.KeyboardAccelerators.Add(escAcc);
    }

    private string? ResolveRepositoryRootForDocument(string canonicalPath)
    {
        if (!string.IsNullOrWhiteSpace(_activeRepositoryRoot)
            && _canonicalizer.IsWithinRoot(canonicalPath, _activeRepositoryRoot))
        {
            return _activeRepositoryRoot;
        }

        return _repoService.DetectRepositoryRoot(canonicalPath);
    }

    private async Task<FileReadResult> ReadDocumentAsync(string canonicalPath, CancellationToken cancellationToken = default)
    {
        var fileInfo = new FileInfo(canonicalPath);
        if (fileInfo.Exists
            && _documentCache.TryGetSource(canonicalPath, fileInfo.LastWriteTimeUtc, fileInfo.Length, out FileReadResult? cached)
            && cached is not null)
        {
            return cached;
        }

        FileReadResult result = await _fileService.ReadTextAsync(canonicalPath, cancellationToken);
        var currentFileInfo = new FileInfo(canonicalPath);
        if (currentFileInfo.Exists)
        {
            _documentCache.SetSource(canonicalPath, currentFileInfo.LastWriteTimeUtc, result);
        }

        return result;
    }

    private async Task<RenderedDocument> RenderDocumentAsync(
        FileReadResult readResult,
        RenderContext context,
        CancellationToken cancellationToken = default)
    {
        var key = new DocumentRenderCacheKey(
            readResult.ContentHash,
            context.SourcePath,
            context.RepositoryRoot,
            context.Theme,
            context.EnableDiagrams,
            context.EnableMath,
            context.AllowRemoteAssets,
            RendererConfigurationVersion);

        if (_documentCache.TryGetRendered(key, out RenderedDocument? cached) && cached is not null)
        {
            return cached;
        }

        RenderedDocument rendered = await _renderer.RenderAsync(readResult.Content, context, cancellationToken);
        _documentCache.SetRendered(key, rendered);
        return rendered;
    }

    public async Task OpenDocumentInTabAsync(string filePath, string? targetAnchor = null, bool recordHistory = true, bool reuseActiveTab = false)
    {
        LoadingOverlay.Visibility = Visibility.Visible;
        try
        {
            await OpenDocumentInTabCoreAsync(filePath, targetAnchor, recordHistory, reuseActiveTab);
        }
        catch (DocumentTooLargeException ex)
        {
            ShowDiagnostic(
                $"This document is too large to render safely ({FormatByteCount(ex.SizeBytes)}). The limit is {FormatByteCount(ex.MaximumBytes)}.",
                DiagnosticSeverity.Warning);
        }
        catch (Exception ex)
        {
            ShowDiagnostic($"Unable to open document: {ex.Message}", DiagnosticSeverity.Error);
        }
        finally
        {
            LoadingOverlay.Visibility = Visibility.Collapsed;
        }
    }

    private void ConfigureWindowIcon()
    {
        try
        {
            IntPtr hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            WindowId windowId = Win32Interop.GetWindowIdFromWindow(hwnd);
            AppWindow appWindow = AppWindow.GetFromWindowId(windowId);
            string iconPath = Path.Combine(AppContext.BaseDirectory, "Assets", "app.ico");
            if (File.Exists(iconPath)) appWindow.SetIcon(iconPath);
        }
        catch (Exception ex)
        {
            LogNonFatal("WindowIcon", ex);
        }
    }

    private static void LogNonFatal(string tag, Exception ex)
    {
        try
        {
            string directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Marknexia");
            Directory.CreateDirectory(directory);
            File.AppendAllText(Path.Combine(directory, "diagnostics.log"), $"[{DateTime.Now:O}] [{tag}] {ex}\n");
        }
        catch { }
    }

    private async Task InitializeAfterLoadAsync()
    {
        if (_hasProcessedStartup) return;
        _hasProcessedStartup = true;

        try
        {
            string? startupPath = StartupFileResolver.FindFirstExisting(
                [_startupFilePath, .. Environment.GetCommandLineArgs().Skip(1)]);
            if (startupPath != null)
            {
                await OpenDocumentInTabAsync(startupPath);
            }
            else
            {
                await RestoreSavedRepositoryRootAsync();
            }
        }
        catch (Exception ex)
        {
            ShowDiagnostic($"Error loading startup document: {ex.Message}", DiagnosticSeverity.Warning);
        }
    }

    private async Task RestoreSavedRepositoryRootAsync()
    {
        string? savedRoot = _settingsService.Current.RepositoryRoot;
        if (string.IsNullOrWhiteSpace(savedRoot)) return;

        try
        {
            string canonicalRoot = _canonicalizer.CanonicalizePath(savedRoot);
            if (!_fileService.DirectoryExists(canonicalRoot))
            {
                _settingsService.Current.RepositoryRoot = null;
                _settingsService.Save(_settingsService.Current);
                return;
            }

            _activeRepositoryRoot = canonicalRoot;
            _settingsService.Current.RepositoryRoot = canonicalRoot;
            await RefreshRepositoryFilesAsync(canonicalRoot);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or ArgumentException)
        {
            _settingsService.Current.RepositoryRoot = null;
            _settingsService.Save(_settingsService.Current);
            ShowDiagnostic("The previously opened repository could not be restored.", DiagnosticSeverity.Warning);
        }
    }

    private async Task OpenDocumentInTabCoreAsync(string filePath, string? targetAnchor, bool recordHistory, bool reuseActiveTab)
    {
        string canonicalPath = _canonicalizer.CanonicalizePath(filePath);

        if (!_fileService.FileExists(canonicalPath))
        {
            ShowDiagnostic($"File not found: {canonicalPath}", DiagnosticSeverity.Error);
            return;
        }

        _settingsService.AddRecentFile(canonicalPath);
        RefreshRecentFiles();

        // Keep the shell's selected workspace for the sidebar, but resolve the
        // asset authority per document so tabs cannot borrow another tab's root.
        string? repositoryRoot = ResolveRepositoryRootForDocument(canonicalPath);
        if (string.IsNullOrEmpty(_activeRepositoryRoot) && !string.IsNullOrEmpty(repositoryRoot))
        {
            _activeRepositoryRoot = repositoryRoot;
            _settingsService.Current.RepositoryRoot = repositoryRoot;
            _settingsService.Save(_settingsService.Current);
            await RefreshRepositoryFilesAsync(_activeRepositoryRoot);
        }

        // Check if tab already exists
        foreach (var state in _tabStates)
        {
            if (!reuseActiveTab && state.FilePath.Equals(canonicalPath, StringComparison.OrdinalIgnoreCase))
            {
                SelectDocumentTab(state);
                bool locationChanged = !string.Equals(state.CurrentFragment, targetAnchor, StringComparison.OrdinalIgnoreCase);
                if (locationChanged && recordHistory)
                {
                    state.History.Push(new HistoryEntry(
                        new DocumentUri(state.FilePath, state.CurrentFragment),
                        state.CurrentFragment,
                        0,
                        DateTimeOffset.UtcNow));
                }

                state.CurrentFragment = targetAnchor;
                if (locationChanged)
                {
                    string script = string.IsNullOrEmpty(targetAnchor)
                        ? "window.marknexiaBridge?.scrollToTop()"
                        : $"window.marknexiaBridge?.scrollToAnchor({JsArg(targetAnchor)})";
                    await state.WebView.ExecuteScriptAsync(script);
                }
                UpdateButtonStates();
                return;
            }
        }

        if (reuseActiveTab && GetActiveTabState() is DocumentTabState activeTab)
        {
            await ReplaceActiveDocumentAsync(activeTab, canonicalPath, targetAnchor, recordHistory);
            return;
        }

        // Read and Render
        FileReadResult readResult = await ReadDocumentAsync(canonicalPath);
        AppTheme currentTheme = GetCurrentTheme();
        var renderContext = new RenderContext(canonicalPath, repositoryRoot, currentTheme, AllowRemoteAssets: _settingsService.Current.AllowRemoteAssets);
        RenderedDocument rendered = await RenderDocumentAsync(readResult, renderContext);

        // Create Tab & WebView2
        var webView = new WebView2
        {
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            DefaultBackgroundColor = currentTheme == AppTheme.Dark
                ? Windows.UI.Color.FromArgb(255, 13, 17, 23)
                : Windows.UI.Color.FromArgb(255, 255, 255, 255)
        };

        var tabState = new DocumentTabState(canonicalPath, readResult.Content, rendered, webView, repositoryRoot)
        {
            CurrentFragment = targetAnchor,
            PendingScroll = true
        };
        _tabStates.Add(tabState);
        AddDocumentTab(tabState);
        SelectDocumentTab(tabState);

        try { await PrepareWebViewAsync(webView, canonicalPath, rendered); }
        catch (Exception ex)
        {
            RemoveDocumentTab(tabState);
            SelectDocumentTab(_tabStates.LastOrDefault());
            ShowDiagnostic($"WebView2 initialization failed: {ex.Message}", DiagnosticSeverity.Error);
            UpdateButtonStates();
            return;
        }

        UpdateOutlineList(rendered.Headings);

        // The initial location is not a back-stack entry. Subsequent navigation
        // records the previous location in the existing tab branch above.
        UpdateButtonStates();
    }

    private async Task ReplaceActiveDocumentAsync(DocumentTabState state, string canonicalPath, string? targetAnchor, bool recordHistory)
    {
        FileReadResult readResult = await ReadDocumentAsync(canonicalPath);
        string? repositoryRoot = ResolveRepositoryRootForDocument(canonicalPath);
        RenderedDocument rendered = await RenderDocumentAsync(
            readResult,
            new RenderContext(canonicalPath, repositoryRoot, GetCurrentTheme(), AllowRemoteAssets: _settingsService.Current.AllowRemoteAssets));

        string previousFilePath = state.FilePath;
        string previousSourceText = state.SourceText;
        RenderedDocument? previousDocument = state.Document;
        string? previousRepositoryRoot = state.RepositoryRoot;
        string? previousFragment = state.CurrentFragment;
        HistoryEntry previousLocation = new(
            new DocumentUri(previousFilePath, previousFragment),
            previousFragment,
            0,
            DateTimeOffset.UtcNow);

        state.FilePath = canonicalPath;
        state.SourceText = readResult.Content;
        state.Document = rendered;
        state.RepositoryRoot = repositoryRoot;
        state.CurrentFragment = targetAnchor;
        state.PendingScroll = true;

        try
        {
            await PrepareWebViewAsync(state.WebView, canonicalPath, rendered);
        }
        catch
        {
            state.FilePath = previousFilePath;
            state.SourceText = previousSourceText;
            state.Document = previousDocument;
            state.RepositoryRoot = previousRepositoryRoot;
            state.CurrentFragment = previousFragment;
            state.PendingScroll = false;
            throw;
        }

        if (recordHistory)
        {
            state.History.Push(previousLocation);
        }

        if (FindDocumentTab(state) is TabViewItem tabItem)
        {
            tabItem.Header = state.Title;
        }
        SelectDocumentTab(state);
        UpdateOutlineList(rendered.Headings);
        UpdateButtonStates();

    }

    private async Task ApplyPendingLocationAsync(DocumentTabState state)
    {
        if (!state.PendingScroll) return;

        state.PendingScroll = false;
        string script = string.IsNullOrEmpty(state.CurrentFragment)
            ? "window.marknexiaBridge?.scrollToTop()"
            : $"window.marknexiaBridge?.scrollToAnchor({JsArg(state.CurrentFragment)})";
        try
        {
            await state.WebView.ExecuteScriptAsync(script);
        }
        catch (Exception ex) when (ex is InvalidOperationException or System.Runtime.InteropServices.COMException)
        {
            ShowDiagnostic("The document loaded, but its requested location could not be restored.", DiagnosticSeverity.Warning);
        }
    }

    private async Task PrepareWebViewAsync(WebView2 webView, string canonicalPath, RenderedDocument rendered)
    {
        InvalidateSearchForNavigation(webView);
        var env = await GetOrCreateWebViewEnvironmentAsync();
        bool initializeMessages = webView.CoreWebView2 == null;
        await webView.EnsureCoreWebView2Async(env);
        CoreWebView2 core = webView.CoreWebView2 ?? throw new InvalidOperationException("WebView2 did not initialize.");
        _assetContextsByOrigin[rendered.AssetContext.Origin] = rendered.AssetContext;
        if (initializeMessages)
        {
            core.WebMessageReceived += CoreWebView2_WebMessageReceived;
            AttachSearchNavigationHandlers(webView, core);
            AttachResourceBroker(core);
        }

        if (rendered.HtmlContent.Contains("https://marknexia.assets/mermaid.min.js", StringComparison.Ordinal))
        {
            string assetsDirectory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Marknexia",
                "Assets");
            TemplateEngine.EnsureAssetsExtracted(assetsDirectory);
            core.SetVirtualHostNameToFolderMapping(
                "marknexia.assets",
                assetsDirectory,
                CoreWebView2HostResourceAccessKind.Allow);
        }

        if (rendered.HtmlContent.Length < 1_500_000)
        {
            try { webView.NavigateToString(rendered.HtmlContent); }
            catch (ArgumentException) { NavigateHtmlViaVirtualHost(webView, rendered.HtmlContent); }
        }
        else
        {
            NavigateHtmlViaVirtualHost(webView, rendered.HtmlContent);
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

        DocumentTabState? state = _tabStates.FirstOrDefault(tab => ReferenceEquals(tab.WebView, webView));
        if (state != null)
        {
            string? previousCacheFilePath = state.RenderCacheFilePath;
            state.RenderCacheFilePath = filePath;
            TryDeleteRenderCacheFile(previousCacheFilePath, cacheDir);
        }

        webView.CoreWebView2.SetVirtualHostNameToFolderMapping(
            "marknexia.page",
            cacheDir,
            CoreWebView2HostResourceAccessKind.Allow);
        webView.CoreWebView2.Navigate($"https://marknexia.page/{fileId}");
    }

    private static void TryDeleteRenderCacheFile(string? filePath, string? cacheDirectory)
    {
        if (string.IsNullOrWhiteSpace(filePath)) return;

        try
        {
            string directory = cacheDirectory ?? Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Marknexia",
                "RenderCache");
            string fullDirectory = Path.TrimEndingDirectorySeparator(Path.GetFullPath(directory));
            string fullPath = Path.GetFullPath(filePath);
            if (!fullPath.StartsWith(fullDirectory + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase)) return;
            File.Delete(fullPath);
        }
        catch
        {
            // Cache cleanup is best effort and must not interrupt navigation.
        }
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
            activeTab?.RepositoryRoot,
            null,
            new NavigationPolicy(_settingsService.Current.EnforceRepositorySandbox, _settingsService.Current.AllowExternalLinks, _settingsService.Current.AllowRemoteAssets));

        NavigationIntent intent = _navResolver.Resolve(destination, context);

        switch (intent.Kind)
        {
            case NavigationKind.SameDocumentAnchor:
                if (!string.IsNullOrEmpty(intent.Fragment) && activeTab != null)
                {
                    await OpenDocumentInTabAsync(activeTab.FilePath, intent.Fragment);
                }
                break;

            case NavigationKind.CrossDocument:
            case NavigationKind.CrossDocumentWithAnchor:
            case NavigationKind.RepositoryRootRelative:
                if (intent.TargetDocument != null)
                {
                    await OpenDocumentInTabAsync(intent.TargetDocument.CanonicalPath, intent.Fragment, reuseActiveTab: true);
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
                _ = OpenDocumentInTabAsync(active.FilePath, item.SlugId);
            }
        }
    }

    private void CopyAnchor_Click(object sender, RoutedEventArgs e)
    {
        if (sender is not Button { Tag: OutlineItemViewModel item }) return;
        var dataPackage = new DataPackage();
        dataPackage.SetText($"#{item.SlugId}");
        Clipboard.SetContent(dataPackage);
        ShowDiagnostic($"Copied #{item.SlugId}", DiagnosticSeverity.Info);
    }

    private void RepositoryTreeView_ItemInvoked(TreeView sender, TreeViewItemInvokedEventArgs args)
    {
        if (args.InvokedItem is TreeViewNode { Content: RepositoryTreeRow file } && !file.IsFolder)
        {
            _ = OpenDocumentInTabAsync(file.FullPath);
        }
    }

    private void RevealFile_Click(object sender, RoutedEventArgs e)
    {
        if (sender is not Button { Tag: RepositoryTreeRow row }) return;
        try
        {
            string target = row.IsFolder ? row.FullPath : $"/select,\"{row.FullPath}\"";
            Process.Start(new ProcessStartInfo("explorer.exe", target) { UseShellExecute = true });
        }
        catch (Exception ex)
        {
            ShowDiagnostic($"Could not open File Explorer: {ex.Message}", DiagnosticSeverity.Warning);
        }
    }

    private async Task RefreshRepositoryFilesAsync(string rootPath)
    {
        _repositoryRefreshCancellation.Cancel();
        _repositoryRefreshCancellation.Dispose();
        _repositoryRefreshCancellation = new CancellationTokenSource();
        CancellationToken cancellationToken = _repositoryRefreshCancellation.Token;

        try
        {
            RepositoryTreeNode tree = await _treeBuilder.BuildAsync(rootPath, cancellationToken);
            cancellationToken.ThrowIfCancellationRequested();
            if (!string.Equals(_activeRepositoryRoot, rootPath, StringComparison.OrdinalIgnoreCase)) return;

            RepositoryTreeView.RootNodes.Clear();
            RepositoryTreeView.RootNodes.Add(BuildTreeNode(tree, isRoot: true));
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // A newer workspace selection superseded this scan.
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or ArgumentException)
        {
            ShowDiagnostic("The repository tree could not be loaded.", DiagnosticSeverity.Warning);
        }
    }

    private static TreeViewNode BuildTreeNode(RepositoryTreeNode node, bool isRoot = false)
    {
        var treeNode = new TreeViewNode
        {
            Content = new RepositoryTreeRow(node, node.IsFolder && isRoot ? -1 : 0),
            IsExpanded = isRoot
        };
        foreach (RepositoryTreeNode child in node.Children)
        {
            treeNode.Children.Add(BuildTreeNode(child));
        }
        return treeNode;
    }

    private DocumentTabState? GetActiveTabState()
    {
        return (DocumentTabsView?.SelectedItem as TabViewItem)?.Tag as DocumentTabState;
    }

    private TabViewItem? FindDocumentTab(DocumentTabState state)
    {
        return DocumentTabsView?.TabItems.OfType<TabViewItem>()
            .FirstOrDefault(item => ReferenceEquals(item.Tag, state));
    }

    private void SelectDocumentTab(DocumentTabState? state)
    {
        DocumentTabsView.SelectedItem = state is null ? null : FindDocumentTab(state);
    }

    private static TabViewItem CreateDocumentTab(DocumentTabState state)
    {
        return new TabViewItem
        {
            Header = state.Title,
            Tag = state,
            IsClosable = true,
            Content = new Grid()
        };
    }

    private void AddDocumentTab(DocumentTabState state)
    {
        _isSynchronizingTabs = true;
        try
        {
            DocumentTabsView.TabItems.Add(CreateDocumentTab(state));
        }
        finally
        {
            _isSynchronizingTabs = false;
        }
    }

    private void RemoveDocumentTab(DocumentTabState state)
    {
        _isSynchronizingTabs = true;
        try
        {
            if (FindDocumentTab(state) is TabViewItem tabItem)
            {
                DocumentTabsView.TabItems.Remove(tabItem);
            }
            _tabStates.Remove(state);
        }
        finally
        {
            _isSynchronizingTabs = false;
        }
    }

    private void DocumentTabs_TabItemsChanged(TabView sender, IVectorChangedEventArgs args)
    {
        if (_isSynchronizingTabs) return;

        var orderedStates = sender.TabItems.OfType<TabViewItem>()
            .Select(item => item.Tag as DocumentTabState)
            .Where(state => state is not null)
            .Cast<DocumentTabState>()
            .ToList();

        if (orderedStates.Count != _tabStates.Count) return;
        _tabStates.Clear();
        _tabStates.AddRange(orderedStates);
    }

    private void DocumentTabs_TabCloseRequested(TabView sender, TabViewTabCloseRequestedEventArgs args)
    {
        if (args.Tab?.Tag is DocumentTabState state)
        {
            CloseTab(state);
        }
    }

    private void CloseCurrentTab()
    {
        if (GetActiveTabState() is DocumentTabState tab)
        {
            CloseTab(tab);
        }
    }

    private void CloseTab(DocumentTabState tab)
    {
        bool wasActive = ReferenceEquals(GetActiveTabState(), tab);
        int closingIndex = DocumentTabsView.TabItems.IndexOf(FindDocumentTab(tab));
        TryDeleteRenderCacheFile(tab.RenderCacheFilePath, null);
        RemoveDocumentTab(tab);

        if (_tabStates.Count > 0)
        {
            if (wasActive)
            {
                SelectDocumentTab(_tabStates[Math.Min(Math.Max(0, closingIndex), _tabStates.Count - 1)]);
            }
        }
        else
        {
            OutlineListView.ItemsSource = null;
            DocumentContentPresenter.Content = null;
            DocumentContentPresenter.Visibility = Visibility.Collapsed;
            WelcomePanel.Visibility = Visibility.Visible;
        }
    }

    private void CloseCurrentTab_MenuClick(object sender, RoutedEventArgs e) => CloseCurrentTab();

    private async void ShowAbout_Click(object sender, RoutedEventArgs e)
    {
        if (Content is not FrameworkElement { XamlRoot: not null } root) return;
        string version = CurrentProductVersion;
        var content = new StackPanel { Spacing = 10, MinWidth = 360 };
        Brush accentBrush = GetThemeBrush(root, "MarknexiaAccentBrush", Windows.UI.Color.FromArgb(255, 6, 182, 212), Windows.UI.Color.FromArgb(255, 34, 211, 238));
        Brush mutedBrush = GetThemeBrush(root, "MarknexiaMutedTextBrush", Windows.UI.Color.FromArgb(255, 100, 116, 139), Windows.UI.Color.FromArgb(255, 148, 163, 184));
        content.Children.Add(new Image { Source = new Microsoft.UI.Xaml.Media.Imaging.BitmapImage(new Uri("ms-appx:///Assets/marknexia-96.png")), Width = 64, Height = 64, HorizontalAlignment = HorizontalAlignment.Center });
        content.Children.Add(new TextBlock { Text = "Marknexia", FontSize = 24, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, HorizontalAlignment = HorizontalAlignment.Center });
        content.Children.Add(new TextBlock { Text = "Read. Explore. Understand.", Foreground = accentBrush, HorizontalAlignment = HorizontalAlignment.Center });
        content.Children.Add(new TextBlock { Text = "GitHub-style Markdown. Native on Windows.", TextAlignment = TextAlignment.Center, TextWrapping = TextWrapping.Wrap });
        content.Children.Add(new TextBlock { Text = $"Version {version}\nOffline-first rendering • Secure local navigation", Foreground = mutedBrush, TextAlignment = TextAlignment.Center, TextWrapping = TextWrapping.Wrap });
        await ShowDialogAsync("About Marknexia", content);
    }

    private static Brush GetThemeBrush(FrameworkElement root, string key, Windows.UI.Color lightFallback, Windows.UI.Color darkFallback)
    {
        if (Application.Current.Resources[key] is Brush brush) return brush;
        return new SolidColorBrush(root.ActualTheme == ElementTheme.Dark ? darkFallback : lightFallback);
    }

    private async void ShowHelp_Click(object sender, RoutedEventArgs e)
    {
        var content = new ScrollViewer { MaxHeight = 420, MaxWidth = 520 };
        var stack = new StackPanel { Spacing = 12 };
        stack.Children.Add(new TextBlock { Text = "Open a Markdown file or drop a file/folder onto the window. Repository mode shows Markdown files and supported assets; select a heading in the document map to jump to it.", TextWrapping = TextWrapping.Wrap });
        stack.Children.Add(new TextBlock { Text = "Keyboard shortcuts", FontSize = 16, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold });
        stack.Children.Add(new TextBlock { Text = "Ctrl+O  Open file\nCtrl+Shift+O  Open repository\nCtrl+T  Open a file picker for a new tab\nCtrl+F  Find\nEnter / Shift+Enter  Next / previous match\nAlt+Left / Alt+Right  Back / forward\nCtrl+Tab / Ctrl+Shift+Tab  Next / previous tab\nCtrl+W  Close tab\nF5 / Ctrl+R  Reload\nEsc  Close search", FontFamily = new Microsoft.UI.Xaml.Media.FontFamily("Cascadia Code"), TextWrapping = TextWrapping.Wrap });
        stack.Children.Add(new TextBlock { Text = "Links stay inside the repository sandbox when enabled. External links require an explicit handoff to the system browser. Rendering assets are bundled locally.", TextWrapping = TextWrapping.Wrap });
        content.Content = stack;
        await ShowDialogAsync("Marknexia help", content);
    }

    private async void CheckForUpdates_Click(object sender, RoutedEventArgs e)
    {
        string currentVersion = CurrentProductVersion;
        UpdateCheckResult result = await _updateService.CheckAsync(currentVersion);
        if (!string.IsNullOrEmpty(result.Error))
        {
            ShowDiagnostic($"Update check unavailable: {result.Error}", DiagnosticSeverity.Warning);
            return;
        }

        if (!result.IsUpdateAvailable)
        {
            await ShowDialogAsync("Marknexia is up to date", new TextBlock { Text = $"You are using Marknexia {currentVersion}.", TextWrapping = TextWrapping.Wrap });
            return;
        }

        bool isPackagedInstallation = IsPackagedInstallation();
        bool canInstall = !isPackagedInstallation && result.PortablePackage != null && result.PortableChecksum != null;
        string architecture = UpdateService.CurrentArchitectureAssetToken;
        var dialog = new ContentDialog
        {
            Title = "Marknexia update available",
            Content = new TextBlock
            {
                Text = isPackagedInstallation
                    ? $"Version {result.LatestVersion} is available. This installation is managed as an MSIX package; use the official release page or Store channel to update it."
                    : canInstall
                    ? $"Version {result.LatestVersion} is available. Marknexia will verify the release checksum, install it safely, and restart."
                    : $"Version {result.LatestVersion} is available. This release does not include an installable {architecture} package, so review it on the official release page.",
                TextWrapping = TextWrapping.Wrap
            },
            PrimaryButtonText = canInstall ? "Download and restart" : "Open release page",
            SecondaryButtonText = canInstall && result.ReleaseUri != null ? "Open release page" : null,
            CloseButtonText = "Later",
            XamlRoot = (Content as FrameworkElement)?.XamlRoot
        };
        ContentDialogResult choice = await dialog.ShowAsync();
        if (choice == ContentDialogResult.Primary && canInstall)
        {
            try
            {
                ShowDiagnostic("Downloading and verifying the Marknexia update…", DiagnosticSeverity.Info);
                StagedUpdate staged = await _updateService.DownloadAndStageAsync(result);
                UpdateService.StartUpdateAndRestart(staged, AppContext.BaseDirectory, Environment.ProcessId);
                Environment.Exit(0);
            }
            catch (Exception ex) when (ex is HttpRequestException or IOException or InvalidDataException or InvalidOperationException)
            {
                ShowDiagnostic($"Update could not be installed: {ex.Message}", DiagnosticSeverity.Warning);
            }
        }
        else if ((choice == ContentDialogResult.Secondary || !canInstall && choice == ContentDialogResult.Primary)
            && result.ReleaseUri != null)
        {
            await Windows.System.Launcher.LaunchUriAsync(result.ReleaseUri);
        }
    }

    private static bool IsPackagedInstallation()
    {
        try
        {
            return Windows.ApplicationModel.Package.Current?.Id != null;
        }
        catch (Exception ex) when (ex is InvalidOperationException or System.Runtime.InteropServices.COMException)
        {
            return false;
        }
    }

    private async Task ShowDialogAsync(string title, object content)
    {
        if ((Content as FrameworkElement)?.XamlRoot == null) return;
        var dialog = new ContentDialog
        {
            Title = title,
            Content = content,
            CloseButtonText = "Close",
            XamlRoot = (Content as FrameworkElement)?.XamlRoot
        };
        await dialog.ShowAsync();
    }

    private void SelectAdjacentTab(bool reverse)
    {
        if (_tabStates.Count < 2) return;
        int current = DocumentTabsView.SelectedIndex;
        int next = reverse ? current - 1 : current + 1;
        if (next < 0) next = _tabStates.Count - 1;
        if (next >= _tabStates.Count) next = 0;
        DocumentTabsView.SelectedIndex = next;
    }

    private void DocumentTabs_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        RefreshSearchForActiveTab();
        var active = GetActiveTabState();
        if (active?.Document != null)
        {
            DocumentContentPresenter.Content = active.WebView;
            DocumentContentPresenter.Visibility = Visibility.Visible;
            WelcomePanel.Visibility = Visibility.Collapsed;
            UpdateOutlineList(active.Document.Headings);
            UpdateButtonStates();
        }
        else
        {
            OutlineListView.ItemsSource = null;
            DocumentContentPresenter.Content = null;
            DocumentContentPresenter.Visibility = Visibility.Collapsed;
            WelcomePanel.Visibility = Visibility.Visible;
            UpdateButtonStates();
        }

        UpdateStartPageAffordance();
    }

    private void UpdateStartPageAffordance()
    {
        bool isStartPage = GetActiveTabState() is null;
        ToolTipService.SetToolTip(
            OpenFileButton,
            isStartPage ? "Open File (Ctrl+O)" : null);
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
            await OpenRepositoryFolderAsync(folder.Path);
        }
    }

    private async Task OpenRepositoryFolderAsync(string folderPath)
    {
        if (!_fileService.DirectoryExists(folderPath))
        {
            ShowDiagnostic($"Folder not found: {folderPath}", DiagnosticSeverity.Error);
            return;
        }

        _activeRepositoryRoot = _canonicalizer.CanonicalizePath(folderPath);
        _settingsService.Current.RepositoryRoot = _activeRepositoryRoot;
        _settingsService.AddRecentFolder(_activeRepositoryRoot);
        await RefreshRepositoryFilesAsync(_activeRepositoryRoot);
        SidebarModeSelector.SelectedIndex = 1;
        MainSplitView.IsPaneOpen = true;

        string readme = Path.Combine(_activeRepositoryRoot, "README.md");
        if (_fileService.FileExists(readme)) await OpenDocumentInTabAsync(readme);
    }

    private void RefreshRecentFiles()
    {
        var files = _settingsService.Current.RecentFiles
            .Where(File.Exists)
            .Take(5)
            .ToList();

        RecentFilesListView.ItemsSource = files;
        bool hasRecentFiles = files.Count > 0;
        RecentFilesLabel.Visibility = hasRecentFiles ? Visibility.Visible : Visibility.Collapsed;
        RecentFilesListView.Visibility = hasRecentFiles ? Visibility.Visible : Visibility.Collapsed;
    }

    private async void RecentFile_ItemClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is string path) await OpenDocumentInTabAsync(path);
    }

    private void Root_DragOver(object sender, DragEventArgs e)
    {
        e.AcceptedOperation = e.DataView.Contains(StandardDataFormats.StorageItems)
            ? DataPackageOperation.Copy
            : DataPackageOperation.None;
    }

    private async void Root_Drop(object sender, DragEventArgs e)
    {
        if (!e.DataView.Contains(StandardDataFormats.StorageItems)) return;

        IReadOnlyList<IStorageItem> items = await e.DataView.GetStorageItemsAsync();
        IStorageItem? item = items.FirstOrDefault();
        switch (item)
        {
            case StorageFile file when RepositoryTreeBuilder.IsSupportedMarkdownFile(file.Path):
                await OpenDocumentInTabAsync(file.Path);
                break;
            case StorageFolder folder:
                await OpenRepositoryFolderAsync(folder.Path);
                break;
            case StorageFile:
                ShowDiagnostic("Marknexia opens Markdown files (.md, .markdown, .mdown, .mkdn).", DiagnosticSeverity.Warning);
                break;
        }
    }

    private async void Reload_Click(object sender, RoutedEventArgs e)
    {
        var active = GetActiveTabState();
        if (active != null)
        {
            RenderedDocument? previousDocument = active.Document;
            string previousSourceText = active.SourceText;
            bool previousPendingScroll = active.PendingScroll;
            try
            {
                FileReadResult readResult = await ReadDocumentAsync(active.FilePath);
                var renderContext = new RenderContext(active.FilePath, active.RepositoryRoot, GetCurrentTheme(), AllowRemoteAssets: _settingsService.Current.AllowRemoteAssets);
                RenderedDocument rendered = await RenderDocumentAsync(readResult, renderContext);
                active.Document = rendered;
                active.SourceText = readResult.Content;
                active.PendingScroll = true;
                AppTheme currentTheme = GetCurrentTheme();
                active.WebView.DefaultBackgroundColor = currentTheme == AppTheme.Dark
                    ? Windows.UI.Color.FromArgb(255, 13, 17, 23)
                    : Windows.UI.Color.FromArgb(255, 255, 255, 255);

                await PrepareWebViewAsync(active.WebView, active.FilePath, rendered);
                UpdateOutlineList(rendered.Headings);
            }
            catch (DocumentTooLargeException ex)
            {
                active.Document = previousDocument;
                active.SourceText = previousSourceText;
                active.PendingScroll = previousPendingScroll;
                ShowDiagnostic(
                    $"This document is too large to render safely ({FormatByteCount(ex.SizeBytes)}). The limit is {FormatByteCount(ex.MaximumBytes)}.",
                    DiagnosticSeverity.Warning);
            }
            catch (Exception ex)
            {
                active.Document = previousDocument;
                active.SourceText = previousSourceText;
                active.PendingScroll = previousPendingScroll;
                ShowDiagnostic($"Unable to reload document: {ex.Message}", DiagnosticSeverity.Error);
            }
        }
    }

    private async void Back_Click(object sender, RoutedEventArgs e)
    {
        var active = GetActiveTabState();
        if (active != null && active.History.CanGoBack)
        {
            var currentEntry = new HistoryEntry(
                new DocumentUri(active.FilePath, active.CurrentFragment),
                active.CurrentFragment,
                0,
                DateTimeOffset.UtcNow);
            var prev = active.History.GoBack(currentEntry);
            if (prev != null)
            {
                await OpenDocumentInTabAsync(prev.Document.CanonicalPath, prev.Fragment, recordHistory: false, reuseActiveTab: true);
            }
            UpdateButtonStates();
        }
    }

    private async void Forward_Click(object sender, RoutedEventArgs e)
    {
        var active = GetActiveTabState();
        if (active != null && active.History.CanGoForward)
        {
            var currentEntry = new HistoryEntry(
                new DocumentUri(active.FilePath, active.CurrentFragment),
                active.CurrentFragment,
                0,
                DateTimeOffset.UtcNow);
            var next = active.History.GoForward(currentEntry);
            if (next != null)
            {
                await OpenDocumentInTabAsync(next.Document.CanonicalPath, next.Fragment, recordHistory: false, reuseActiveTab: true);
            }
            UpdateButtonStates();
        }
    }

    private void UpdateButtonStates()
    {
        var active = GetActiveTabState();
        BackButton.IsEnabled = active?.History.CanGoBack == true;
        ForwardButton.IsEnabled = active?.History.CanGoForward == true;
    }

    private void ToggleSidebar_Click(object sender, RoutedEventArgs e)
    {
        MainSplitView.IsPaneOpen = !MainSplitView.IsPaneOpen;
        _settingsService.Current.IsSidebarOpen = MainSplitView.IsPaneOpen;
        _settingsService.Save(_settingsService.Current);
    }

    private void SidebarResizeHandle_PointerPressed(object sender, PointerRoutedEventArgs e)
    {
        if (!MainSplitView.IsPaneOpen) return;
        _isResizingSidebar = true;
        SidebarResizeHandle.CapturePointer(e.Pointer);
        UpdateSidebarWidth(e);
        e.Handled = true;
    }

    private void SidebarResizeHandle_PointerMoved(object sender, PointerRoutedEventArgs e)
    {
        if (!_isResizingSidebar) return;
        UpdateSidebarWidth(e);
        e.Handled = true;
    }

    private void SidebarResizeHandle_PointerReleased(object sender, PointerRoutedEventArgs e)
    {
        if (!_isResizingSidebar) return;
        UpdateSidebarWidth(e);
        StopSidebarResize();
        e.Handled = true;
    }

    private void SidebarResizeHandle_PointerCaptureLost(object sender, PointerRoutedEventArgs e) => StopSidebarResize();

    private void UpdateSidebarWidth(PointerRoutedEventArgs e)
    {
        Point point = e.GetCurrentPoint(MainSplitView).Position;
        MainSplitView.OpenPaneLength = Math.Clamp(point.X, MinimumSidebarWidth, MaximumSidebarWidth);
    }

    private void StopSidebarResize()
    {
        if (!_isResizingSidebar) return;
        _isResizingSidebar = false;
        SidebarResizeHandle.ReleasePointerCaptures();
        _settingsService.Current.SidebarWidth = MainSplitView.OpenPaneLength;
        _settingsService.Save(_settingsService.Current);
    }

    private void SidebarMode_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_isInitializing) return;
        if (SidebarModeSelector.SelectedIndex < 0) return;
        ApplySidebarModeVisuals();

        if (_settingsService != null)
        {
            _settingsService.Current.SidebarMode = SidebarModeSelector.SelectedIndex;
            _settingsService.Save(_settingsService.Current);
        }
    }

    private void ApplySidebarModeVisuals()
    {
        if (SidebarModeSelector.SelectedIndex == 0)
        {
            OutlineListView.Visibility = Visibility.Visible;
            RepositoryTreeView.Visibility = Visibility.Collapsed;
            DocumentMapHeader.Visibility = Visibility.Visible;
            RepositoryHeader.Visibility = Visibility.Collapsed;
        }
        else
        {
            OutlineListView.Visibility = Visibility.Collapsed;
            RepositoryTreeView.Visibility = Visibility.Visible;
            DocumentMapHeader.Visibility = Visibility.Collapsed;
            RepositoryHeader.Visibility = Visibility.Visible;
        }
    }

    private void SearchBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == Windows.System.VirtualKey.Enter)
        {
            if ((Microsoft.UI.Input.InputKeyboardSource.GetKeyStateForCurrentThread(Windows.System.VirtualKey.Shift) & Windows.UI.Core.CoreVirtualKeyStates.Down) != 0)
                FindPrevious_Click(sender, new RoutedEventArgs());
            else
                FindNext_Click(sender, new RoutedEventArgs());
            e.Handled = true;
        }
    }

    private void ThemeSelector_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_isInitializing || _settingsService == null || ThemeSelector == null) return;

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

    private void RemoteAssetsMenuItem_Click(object sender, RoutedEventArgs e)
    {
        if (_isInitializing) return;

        _settingsService.Current.AllowRemoteAssets = RemoteAssetsMenuItem.IsChecked;
        _settingsService.Save(_settingsService.Current);
        ShowDiagnostic(
            RemoteAssetsMenuItem.IsChecked
                ? "Remote images are enabled for documents rendered from now on."
                : "Remote images are blocked by default.",
            DiagnosticSeverity.Info);

        if (GetActiveTabState() != null)
        {
            Reload_Click(this, new RoutedEventArgs());
        }
    }

    private void ApplySavedTheme()
    {
        ThemeSelector.SelectedIndex = _settingsService.Current.Theme switch
        {
            AppTheme.Light => 1,
            AppTheme.Dark => 2,
            _ => 0
        };
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

    private static string JsArg(string value) => JsonSerializer.Serialize(value);

    private static string FormatByteCount(long bytes)
    {
        const double kibibyte = 1024;
        const double mebibyte = kibibyte * 1024;
        const double gibibyte = mebibyte * 1024;
        return bytes >= gibibyte ? $"{bytes / gibibyte:0.0} GiB"
            : bytes >= mebibyte ? $"{bytes / mebibyte:0.0} MiB"
            : bytes >= kibibyte ? $"{bytes / kibibyte:0.0} KiB"
            : $"{bytes:N0} bytes";
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
    public string Title => Path.GetFileName(FilePath);
    public RenderedDocument? Document { get; set; }
    public string? RepositoryRoot { get; set; }
    public WebView2 WebView { get; set; }
    public string SourceText { get; set; }
    public string? CurrentFragment { get; set; }
    public INavigationHistoryManager History { get; } = new NavigationHistoryManager();
    public DocumentSearchState Search { get; } = new();
    public bool IsDocumentReady { get; set; }
    public ulong NavigationId { get; set; }
    public bool PendingScroll { get; set; }
    public string? RenderCacheFilePath { get; set; }

    public DocumentTabState(string filePath, string sourceText, RenderedDocument? document, WebView2 webView, string? repositoryRoot = null)
    {
        FilePath = filePath;
        SourceText = sourceText;
        Document = document;
        RepositoryRoot = repositoryRoot;
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

public sealed class RepositoryTreeRow
{
    public string DisplayName { get; }
    public string FullPath { get; }
    public bool IsFolder { get; }
    public string IconGlyph { get; }
    public Thickness Margin { get; }

    public RepositoryTreeRow(RepositoryTreeNode node, int depth)
    {
        DisplayName = node.DisplayName;
        FullPath = node.FullPath;
        IsFolder = node.IsFolder;
        IconGlyph = node.IconGlyph;
        Margin = new Thickness(Math.Max(0, depth) * 14, 2, 0, 2);
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
