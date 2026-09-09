using System.Text.Json;
using System.Reflection;
using Marknexia.Core;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Windows.ApplicationModel.DataTransfer;

namespace Marknexia.App;

public sealed partial class MainWindow
{
    private bool _isSbomDialogOpen;
    private bool _isSelfHelpDialogOpen;

    // AssemblyVersion is intentionally stable for binding compatibility. Use
    // the same product version as the build/SBOM for About and update checks.
    private static string CurrentProductVersion => typeof(MainWindow).Assembly
        .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion.Split('+')[0]
        ?? typeof(MainWindow).Assembly.GetName().Version?.ToString(3)
        ?? "0.0.0";

    private async void ShowSbom_Click(object sender, RoutedEventArgs e)
    {
        if (_isSbomDialogOpen || Content is not FrameworkElement { XamlRoot: not null } root) return;
        _isSbomDialogOpen = true;
        try
        {
            string path = Path.Combine(AppContext.BaseDirectory, "marknexia-sbom.spdx.json");
            if (!File.Exists(path))
            {
                ShowDiagnostic("The software components inventory is missing. Reinstall Marknexia from its release package.", DiagnosticSeverity.Warning);
                return;
            }

            string json = await File.ReadAllTextAsync(path);
            using var document = JsonDocument.Parse(json);
            string[] components = document.RootElement.GetProperty("packages").EnumerateArray()
                .Select(package => $"{package.GetProperty("name").GetString()}  {package.GetProperty("versionInfo").GetString()}")
                .Order(StringComparer.OrdinalIgnoreCase)
                .ToArray();
            var contents = new StackPanel { Spacing = 12, MaxWidth = 520 };
            contents.Children.Add(new TextBlock
            {
                Text = "Software components included in this build. Copy the SPDX inventory for dependency and checksum details.",
                TextWrapping = TextWrapping.Wrap
            });
            var list = new ListView { ItemsSource = components, MaxHeight = 320, SelectionMode = ListViewSelectionMode.None };
            AutomationProperties.SetName(list, "Software components and versions");
            contents.Children.Add(list);
            var dialog = new ContentDialog
            {
                Title = "Marknexia software components",
                Content = contents,
                PrimaryButtonText = "Copy SBOM",
                CloseButtonText = "Close",
                XamlRoot = root.XamlRoot,
                RequestedTheme = root.ActualTheme
            };
            if (await dialog.ShowAsync() == ContentDialogResult.Primary)
            {
                var data = new DataPackage();
                data.SetText(json);
                Clipboard.SetContent(data);
                ShowDiagnostic("Software components inventory copied.", DiagnosticSeverity.Info);
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or JsonException or InvalidOperationException or System.Runtime.InteropServices.COMException)
        {
            ShowDiagnostic("The software components inventory could not be opened or copied. Try reopening Marknexia.", DiagnosticSeverity.Warning);
        }
        finally
        {
            _isSbomDialogOpen = false;
        }
    }

    private async void ShowSelfHelp_Click(object sender, RoutedEventArgs e)
    {
        if (_isSelfHelpDialogOpen || Content is not FrameworkElement { XamlRoot: not null } root) return;
        _isSelfHelpDialogOpen = true;
        try
        {
            string dataDirectory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "Marknexia");
            string diagnostics = string.Join(Environment.NewLine,
                "Marknexia diagnostics",
                $"Version: {CurrentProductVersion}",
                $"OS: {Environment.OSVersion.VersionString}",
                $"Architecture: {System.Runtime.InteropServices.RuntimeInformation.ProcessArchitecture}",
                $"App path: {AppContext.BaseDirectory}",
                $"Local data: {dataDirectory}",
                $"WebView2 data: {Path.Combine(dataDirectory, "WebView2Data")}",
                $"SBOM present: {File.Exists(Path.Combine(AppContext.BaseDirectory, "marknexia-sbom.spdx.json"))}",
                $"Remote images enabled: {_settingsService.Current.AllowRemoteAssets}");

            var contents = new StackPanel { Spacing = 12, MaxWidth = 560 };
            contents.Children.Add(new TextBlock
            {
                Text = "Common fixes",
                FontSize = 16,
                FontWeight = Microsoft.UI.Text.FontWeights.SemiBold
            });
            contents.Children.Add(new TextBlock
            {
                Text = "If a document looks stale, press F5. If an image is unavailable, confirm its path is relative to the Markdown file and that the file is inside the opened workspace. Remote images are blocked unless enabled in Settings. For startup or rendering problems, reopen the file and use the diagnostics copy below when reporting the issue.",
                TextWrapping = TextWrapping.Wrap
            });
            contents.Children.Add(new TextBlock
            {
                Text = "Local data and WebView2 cache are kept under the Marknexia folder. Closing Marknexia before moving or deleting that cache is recommended.",
                TextWrapping = TextWrapping.Wrap
            });

            var dialog = new ContentDialog
            {
                Title = "Marknexia troubleshooting",
                Content = contents,
                PrimaryButtonText = "Copy diagnostics",
                SecondaryButtonText = "Open data folder",
                CloseButtonText = "Close",
                XamlRoot = root.XamlRoot,
                RequestedTheme = root.ActualTheme
            };
            ContentDialogResult choice = await dialog.ShowAsync();
            if (choice == ContentDialogResult.Primary)
            {
                var data = new DataPackage();
                data.SetText(diagnostics);
                Clipboard.SetContent(data);
                ShowDiagnostic("Diagnostics copied.", DiagnosticSeverity.Info);
            }
            else if (choice == ContentDialogResult.Secondary)
            {
                Directory.CreateDirectory(dataDirectory);
                await Windows.System.Launcher.LaunchFolderPathAsync(dataDirectory);
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or System.Runtime.InteropServices.COMException)
        {
            ShowDiagnostic("Troubleshooting information could not be opened. Try reopening Marknexia.", DiagnosticSeverity.Warning);
        }
        finally
        {
            _isSelfHelpDialogOpen = false;
        }
    }
}
