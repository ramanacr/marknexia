# Modern WinUI/Fluent Wizard Installer for Windows Applications
## Master Agent Directive & Implementation Prompt

> **Purpose**: Use this prompt to instruct any AI coding agent (Claude, Gemini, ChatGPT, Antigravity, Copilot, Cursor) to build a production-grade, zero-dependency, WinUI/Fluent-styled wizard installer and uninstaller for any Windows desktop application (.NET, WPF, WinUI, C++, Rust, Go, Electron, Python).

***

```markdown
# Role & Goal
You are an expert Windows Systems & Desktop Application Engineer. Your task is to build a modern, zero-dependency, WinUI/Fluent-styled wizard installer and uninstaller for a Windows desktop application.

Do NOT use external installer frameworks (e.g., Inno Setup, WiX Toolset, NSIS). Instead, implement a self-contained, native .NET/WPF installer executable (`<AppName>Setup.exe`) that embeds the application binaries as an embedded resource and executes with zero external dependencies.

---

## 1. Core Architectural Pillars

1. **Zero External Dependencies**:
   - The installer is a single, standalone executable (`<AppName>Setup.exe`) built on .NET (`net8.0-windows` / `net9.0-windows`).
   - The pre-packaged application binaries are zipped into `Payload.zip` and embedded into the installer binary via `<EmbeddedResource Include="Payload.zip" />`.
   - No external WiX, InnoSetup, or MSI toolchains are required on the host or build server.

2. **Non-Elevated (Per-User) Standard Installation by Default**:
   - Default install location: `%LOCALAPPDATA%\Programs\<AppName>` (`Environment.SpecialFolder.LocalApplicationData`).
   - Does not require mandatory administrator/UAC privileges. Standard users can install, update, and uninstall seamlessly.
   - All shell associations, uninstaller entries, and file handlers are written under `HKEY_CURRENT_USER\Software` (`Registry.CurrentUser`).

3. **Multi-Step Fluent / WinUI Wizard Experience**:
   - **Step 1 (Configuration Panel)**: Header branding banner, target folder selection with modern `OpenFolderDialog`, shortcut toggles (Desktop, Start Menu), file association options, and optional component selections.
   - **Step 2 (Progress Panel)**: Real-time progress bar with dynamic step descriptions (preparing, extracting, configuring shortcuts, registering uninstaller).
   - **Step 3 (Completion Panel)**: Success confirmation, "Launch Application now" option, and "Finish" action button.

4. **Resilient Process Management & Lock Prevention**:
   - Before extracting payload files, the installer checks for running instances of `<AppName>.exe` located *specifically in the target installation folder*.
   - Attempts graceful shutdown first (`proc.CloseMainWindow()`, wait 3 seconds) before falling back to `proc.Kill()` to prevent `IOException` file-locking errors during upgrades.

5. **Windows Shell Integration**:
   - Creates Windows `.lnk` shortcuts using COM `WScript.Shell` with explicit `TargetPath`, `WorkingDirectory`, `Description`, and `IconLocation`.
   - Registers file extensions (e.g. `.pdf`, `.appdata`), `OpenWithProgids`, `PerceivedType`, shell verbs (`open`, `print`), and `DefaultIcon`.

6. **Dual Installer / Uninstaller Architecture**:
   - During installation, the installer copies itself into `<InstallDir>\Uninstall.exe`.
   - Registers the application in Windows **Installed Apps / Add or Remove Programs** (`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<AppName>`) with `DisplayName`, `DisplayVersion`, `Publisher`, `InstallLocation`, `DisplayIcon`, and `UninstallString = "<InstallDir>\Uninstall.exe /uninstall"`.
   - The `/uninstall` CLI switch handles uninstallation: deletes shortcuts, removes registry keys, and deletes the install directory via delayed background cmd execution (`timeout /t 2 & rmdir /s /q "<dir>"`).

---

## 2. Complete File Implementations

### File 1: Project File (`<AppName>Installer.csproj`)
```xml
<Project Sdk="Microsoft.NET.Sdk">

  <PropertyGroup>
    <OutputType>WinExe</OutputType>
    <TargetFramework>net9.0-windows</TargetFramework>
    <Nullable>enable</Nullable>
    <ImplicitUsings>enable</ImplicitUsings>
    <UseWPF>true</UseWPF>
    <RootNamespace>AppName.Installer</RootNamespace>
    <AssemblyName>AppNameSetup</AssemblyName>
    <ApplicationIcon>assets\app_icon.ico</ApplicationIcon>
  </PropertyGroup>

  <ItemGroup>
    <Resource Include="assets\app_icon.ico" />
    <Resource Include="assets\app_icon.png" />
    <Resource Include="assets\file_icon.ico" />
    <Resource Include="assets\file_icon.png" />
  </ItemGroup>

  <ItemGroup>
    <EmbeddedResource Include="Payload.zip" Condition="Exists('Payload.zip')" />
  </ItemGroup>

</Project>
```

---

### File 2: Wizard Window XAML (`InstallerWindow.xaml`)
```xml
<Window x:Class="AppName.Installer.InstallerWindow"
        xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        Title="AppName Setup"
        Icon="pack://application:,,,/assets/app_icon.png"
        Width="540"
        Height="420"
        WindowStartupLocation="CenterScreen"
        ResizeMode="NoResize"
        Background="#F9F9FB"
        FontFamily="Segoe UI, -apple-system, BlinkMacSystemFont, Roboto, sans-serif">

    <Grid>
        <Grid.RowDefinitions>
            <RowDefinition Height="90" />
            <RowDefinition Height="*" />
            <RowDefinition Height="Auto" />
        </Grid.RowDefinitions>

        <!-- Header Branding Banner -->
        <Border Grid.Row="0" Background="#0066CC" Padding="20,12">
            <StackPanel Orientation="Horizontal" VerticalAlignment="Center">
                <Image Source="pack://application:,,,/assets/app_icon.png" Width="54" Height="54" Margin="0,0,16,0" RenderOptions.BitmapScalingMode="HighQuality" />
                <StackPanel VerticalAlignment="Center">
                    <TextBlock Text="AppName Desktop Setup" FontSize="18" FontWeight="Bold" Foreground="White" />
                    <TextBlock Text="Installs AppName Native on your computer" FontSize="12" Foreground="#D0E4FF" Margin="0,3,0,0" />
                </StackPanel>
            </StackPanel>
        </Border>

        <!-- Step 1: Configuration Panel -->
        <Grid Grid.Row="1" x:Name="ConfigStepPanel" Margin="24,20" Visibility="Visible">
            <Grid.RowDefinitions>
                <RowDefinition Height="Auto" />
                <RowDefinition Height="Auto" />
                <RowDefinition Height="Auto" />
                <RowDefinition Height="*" />
            </Grid.RowDefinitions>

            <!-- Destination Folder Selection -->
            <StackPanel Grid.Row="0" Margin="0,0,0,16">
                <TextBlock Text="Installation Destination Folder:" FontWeight="SemiBold" Margin="0,0,0,6" Foreground="#1A1A1A" />
                <Grid>
                    <Grid.ColumnDefinitions>
                        <ColumnDefinition Width="*" />
                        <ColumnDefinition Width="Auto" />
                    </Grid.ColumnDefinitions>
                    <TextBox x:Name="TargetDirectoryBox" Height="30" Padding="8,4" VerticalContentAlignment="Center" Margin="0,0,8,0" FontSize="12" />
                    <Button Grid.Column="1" Content="Browse..." Width="80" Height="30" Click="BrowseFolder_Click" />
                </Grid>
            </StackPanel>

            <!-- Options Group -->
            <GroupBox Grid.Row="1" Header="Shortcuts &amp; Associations" Padding="10" Margin="0,0,0,12">
                <StackPanel>
                    <CheckBox x:Name="DesktopShortcutCheck" Content="Create Desktop shortcut" IsChecked="True" Margin="0,0,0,8" />
                    <CheckBox x:Name="StartMenuShortcutCheck" Content="Create Start Menu shortcut" IsChecked="True" Margin="0,0,0,8" />
                    <CheckBox x:Name="AssociateFilesCheck" Content="Associate with supported file types" IsChecked="True" />
                </StackPanel>
            </GroupBox>

            <!-- Feature Highlight Card -->
            <Border Grid.Row="2" Background="#E6F0FA" BorderBrush="#B8D5FA" BorderThickness="1" CornerRadius="4" Padding="10,8">
                <TextBlock Text="✔ Native high-performance application bundled."
                           Foreground="#0052A3" FontSize="12" FontWeight="SemiBold" />
            </Border>
        </Grid>

        <!-- Step 2: Progress Panel -->
        <Grid Grid.Row="1" x:Name="ProgressStepPanel" Margin="24,30" Visibility="Collapsed">
            <StackPanel VerticalAlignment="Center">
                <TextBlock Text="Installing AppName..." FontSize="16" FontWeight="SemiBold" Margin="0,0,0,8" />
                <TextBlock x:Name="ProgressStatusText" Text="Extracting packaged application files..." Foreground="#666666" Margin="0,0,0,16" />
                <ProgressBar x:Name="InstallProgressBar" Height="14" Minimum="0" Maximum="100" Value="0" />
            </StackPanel>
        </Grid>

        <!-- Step 3: Complete Panel -->
        <Grid Grid.Row="1" x:Name="CompleteStepPanel" Margin="24,30" Visibility="Collapsed">
            <StackPanel VerticalAlignment="Center">
                <TextBlock Text="✔ Installation Successful!" FontSize="20" FontWeight="Bold" Foreground="#107C41" Margin="0,0,0,8" />
                <TextBlock Text="AppName has been installed on your system and is ready for use." Foreground="#333333" FontSize="13" Margin="0,0,0,20" />
                <CheckBox x:Name="LaunchAppCheck" Content="Launch AppName now" IsChecked="True" FontSize="13" FontWeight="SemiBold" />
            </StackPanel>
        </Grid>

        <!-- Bottom Action Bar -->
        <Border Grid.Row="2" Background="#EFEFEF" BorderBrush="#E0E0E0" BorderThickness="0,1,0,0" Padding="16,12">
            <StackPanel Orientation="Horizontal" HorizontalAlignment="Right">
                <Button x:Name="CancelButton" Content="Cancel" Width="85" Height="30" Margin="0,0,8,0" Click="CancelButton_Click" />
                <Button x:Name="ActionNextButton" Content="Install" Width="95" Height="30" Background="#0066CC" Foreground="White" BorderThickness="0" FontWeight="SemiBold" Click="ActionNextButton_Click" />
            </StackPanel>
        </Border>
    </Grid>
</Window>
```

---

### File 3: Wizard Window Code-Behind (`InstallerWindow.xaml.cs`)
```csharp
using System;
using System.Diagnostics;
using System.IO;
using System.Threading.Tasks;
using System.Windows;
using Microsoft.Win32;

namespace AppName.Installer;

public partial class InstallerWindow : Window
{
    private bool _isComplete;

    public InstallerWindow()
    {
        InitializeComponent();
        TargetDirectoryBox.Text = InstallService.DefaultInstallPath;
    }

    private void BrowseFolder_Click(object sender, RoutedEventArgs e)
    {
        var dialog = new OpenFolderDialog
        {
            Title = "Select Installation Directory",
            InitialDirectory = TargetDirectoryBox.Text
        };

        if (dialog.ShowDialog() == true)
        {
            TargetDirectoryBox.Text = dialog.FolderName;
        }
    }

    private async void ActionNextButton_Click(object sender, RoutedEventArgs e)
    {
        if (_isComplete)
        {
            if (LaunchAppCheck.IsChecked == true)
            {
                string exePath = Path.Combine(TargetDirectoryBox.Text, "AppName.exe");
                if (File.Exists(exePath))
                {
                    Process.Start(new ProcessStartInfo
                    {
                        FileName = exePath,
                        UseShellExecute = true
                    });
                }
            }
            Close();
            return;
        }

        string targetDir = TargetDirectoryBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(targetDir))
        {
            MessageBox.Show("Please enter a valid destination folder.", "Invalid Folder", MessageBoxButton.OK, MessageBoxImage.Warning);
            return;
        }

        // Switch to progress screen
        ConfigStepPanel.Visibility = Visibility.Collapsed;
        ProgressStepPanel.Visibility = Visibility.Visible;
        ActionNextButton.IsEnabled = false;
        CancelButton.IsEnabled = false;

        bool desktop = DesktopShortcutCheck.IsChecked == true;
        bool startMenu = StartMenuShortcutCheck.IsChecked == true;
        bool associate = AssociateFilesCheck.IsChecked == true;

        try
        {
            var progress = new Progress<int>(pct =>
            {
                InstallProgressBar.Value = pct;
                ProgressStatusText.Text = pct switch
                {
                    < 30 => "Preparing installation package...",
                    < 75 => $"Extracting application binaries... ({pct}%)",
                    < 85 => "Configuring Windows shortcuts and associations...",
                    _ => "Finalizing setup registration..."
                };
            });

            await Task.Run(() =>
            {
                InstallService.Install(targetDir, desktop, startMenu, associate, progress);
            });

            // Switch to completion screen
            ProgressStepPanel.Visibility = Visibility.Collapsed;
            CompleteStepPanel.Visibility = Visibility.Visible;
            ActionNextButton.Content = "Finish";
            ActionNextButton.IsEnabled = true;
            CancelButton.Visibility = Visibility.Collapsed;
            _isComplete = true;
        }
        catch (Exception ex)
        {
            MessageBox.Show($"Installation failed:\n{ex.Message}", "Installation Error", MessageBoxButton.OK, MessageBoxImage.Error);
            ConfigStepPanel.Visibility = Visibility.Visible;
            ProgressStepPanel.Visibility = Visibility.Collapsed;
            ActionNextButton.IsEnabled = true;
            CancelButton.IsEnabled = true;
        }
    }

    private void CancelButton_Click(object sender, RoutedEventArgs e)
    {
        Close();
    }
}
```

---

### File 4: Installer & Uninstaller Service (`InstallService.cs`)
```csharp
using System;
using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Reflection;
using Microsoft.Win32;

namespace AppName.Installer;

public static class InstallService
{
    public static string DefaultInstallPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "Programs",
        "AppName");

    public static string StartMenuShortcutPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.Programs),
        "AppName.lnk");

    public static string DesktopShortcutPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),
        "AppName.lnk");

    public static void Install(
        string targetDirectory,
        bool createDesktopShortcut,
        bool createStartMenuShortcut,
        bool associateFiles,
        IProgress<int>? progress = null)
    {
        progress?.Report(10);

        // 1. Terminate existing running instances in target directory
        try
        {
            foreach (var proc in Process.GetProcessesByName("AppName"))
            {
                try
                {
                    string? procPath = proc.MainModule?.FileName;
                    if (string.IsNullOrEmpty(procPath) ||
                        !procPath.StartsWith(targetDirectory, StringComparison.OrdinalIgnoreCase))
                    {
                        continue;
                    }

                    if (!proc.CloseMainWindow() || !proc.WaitForExit(3000))
                    {
                        proc.Kill();
                        proc.WaitForExit(2000);
                    }
                }
                catch { }
            }
        }
        catch { }

        Directory.CreateDirectory(targetDirectory);

        // 2. Extract Embedded Payload.zip
        var assemblies = new[] { Assembly.GetExecutingAssembly(), Assembly.GetEntryAssembly(), typeof(InstallService).Assembly };
        Stream? zipStream = null;

        foreach (var asm in assemblies)
        {
            if (asm == null) continue;
            foreach (var name in asm.GetManifestResourceNames())
            {
                if (name.EndsWith("Payload.zip", StringComparison.OrdinalIgnoreCase))
                {
                    zipStream = asm.GetManifestResourceStream(name);
                    break;
                }
            }
            if (zipStream != null) break;
        }

        if (zipStream == null)
            throw new InvalidOperationException("Embedded Payload.zip not found in installer binary.");

        progress?.Report(30);

        using (var archive = new ZipArchive(zipStream, ZipArchiveMode.Read))
        {
            int totalEntries = archive.Entries.Count;
            int current = 0;

            foreach (var entry in archive.Entries)
            {
                if (string.IsNullOrEmpty(entry.Name))
                {
                    Directory.CreateDirectory(Path.Combine(targetDirectory, entry.FullName));
                    continue;
                }

                string destinationPath = Path.Combine(targetDirectory, entry.FullName);
                string? destDir = Path.GetDirectoryName(destinationPath);
                if (!string.IsNullOrEmpty(destDir)) Directory.CreateDirectory(destDir);

                entry.ExtractToFile(destinationPath, overwrite: true);
                current++;
                int pct = 30 + (int)((double)current / totalEntries * 45.0);
                progress?.Report(pct);
            }
        }

        progress?.Report(75);

        string mainExePath = Path.Combine(targetDirectory, "AppName.exe");
        string uninstallerPath = Path.Combine(targetDirectory, "Uninstall.exe");

        // 3. Stage Self as Uninstaller
        try
        {
            string currentExe = Process.GetCurrentProcess().MainModule?.FileName ?? string.Empty;
            if (File.Exists(currentExe))
            {
                File.Copy(currentExe, uninstallerPath, overwrite: true);
            }
        }
        catch { }

        // 4. Create Shortcuts
        if (createStartMenuShortcut)
        {
            CreateShortcut(StartMenuShortcutPath, mainExePath, "AppName Desktop Application");
        }

        if (createDesktopShortcut)
        {
            CreateShortcut(DesktopShortcutPath, mainExePath, "AppName Desktop Application");
        }

        progress?.Report(85);

        // 5. Register in Windows Add/Remove Programs
        RegisterUninstaller(targetDirectory, mainExePath, uninstallerPath);

        // 6. Register File Associations
        if (associateFiles)
        {
            RegisterFileAssociations(mainExePath);
        }

        progress?.Report(100);
    }

    public static void CreateShortcut(string shortcutPath, string targetExePath, string description)
    {
        try
        {
            Type? shellType = Type.GetTypeFromProgID("WScript.Shell");
            if (shellType != null)
            {
                dynamic shell = Activator.CreateInstance(shellType)!;
                dynamic shortcut = shell.CreateShortcut(shortcutPath);
                shortcut.TargetPath = targetExePath;
                shortcut.IconLocation = $"{targetExePath},0";
                shortcut.WorkingDirectory = Path.GetDirectoryName(targetExePath);
                shortcut.Description = description;
                shortcut.Save();
            }
        }
        catch { }
    }

    private static void RegisterUninstaller(string installDir, string mainExePath, string uninstallerPath)
    {
        try
        {
            string version = typeof(InstallService).Assembly.GetName().Version?.ToString(3) ?? "1.0.0";
            if (File.Exists(mainExePath))
            {
                var fvi = FileVersionInfo.GetVersionInfo(mainExePath);
                if (!string.IsNullOrEmpty(fvi.ProductVersion))
                {
                    version = fvi.ProductVersion.Split('+')[0];
                }
            }

            using var key = Registry.CurrentUser.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\AppName");
            if (key != null)
            {
                key.SetValue("DisplayName", "AppName Desktop");
                key.SetValue("DisplayVersion", version);
                key.SetValue("Publisher", "AppName Publisher");
                key.SetValue("InstallLocation", installDir);
                key.SetValue("DisplayIcon", mainExePath);
                key.SetValue("UninstallString", $"\"{uninstallerPath}\" /uninstall");
                key.SetValue("NoModify", 1, RegistryValueKind.DWord);
                key.SetValue("NoRepair", 1, RegistryValueKind.DWord);
            }
        }
        catch { }
    }

    private static void RegisterFileAssociations(string mainExePath)
    {
        try
        {
            string installDir = Path.GetDirectoryName(mainExePath) ?? string.Empty;
            string fileIconPath = Path.Combine(installDir, "assets", "file_icon.ico");
            string iconRef = File.Exists(fileIconPath) ? $"{fileIconPath},0" : $"{mainExePath},0";

            // Register File Extension
            using (var extKey = Registry.CurrentUser.CreateSubKey(@"Software\Classes\.ext"))
            {
                if (extKey != null)
                {
                    extKey.SetValue(string.Empty, "AppName.Document");
                    extKey.SetValue("Content Type", "application/octet-stream");
                    extKey.SetValue("PerceivedType", "document");

                    using var openWith = extKey.CreateSubKey("OpenWithProgids");
                    openWith?.SetValue("AppName.Document", string.Empty);
                }
            }

            // Register ProgId
            using (var progKey = Registry.CurrentUser.CreateSubKey(@"Software\Classes\AppName.Document"))
            {
                if (progKey != null)
                {
                    progKey.SetValue(string.Empty, "AppName Document");
                    progKey.SetValue("FriendlyTypeName", "AppName Document");

                    using var iconKey = progKey.CreateSubKey("DefaultIcon");
                    iconKey?.SetValue(string.Empty, iconRef);

                    using var cmdKey = progKey.CreateSubKey(@"shell\open\command");
                    cmdKey?.SetValue(string.Empty, $"\"{mainExePath}\" \"%1\"");
                }
            }
        }
        catch { }
    }

    public static void Uninstall()
    {
        // 1. Remove shortcuts
        try
        {
            if (File.Exists(StartMenuShortcutPath)) File.Delete(StartMenuShortcutPath);
            if (File.Exists(DesktopShortcutPath)) File.Delete(DesktopShortcutPath);
        }
        catch { }

        // 2. Remove Registry entries
        try
        {
            Registry.CurrentUser.DeleteSubKeyTree(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\AppName", throwOnMissingSubKey: false);
            Registry.CurrentUser.DeleteSubKeyTree(@"Software\Classes\AppName.Document", throwOnMissingSubKey: false);
        }
        catch { }

        // 3. Self-delete install directory via background cmd
        string currentExe = Process.GetCurrentProcess().MainModule?.FileName ?? string.Empty;
        string installDir = Path.GetDirectoryName(currentExe) ?? DefaultInstallPath;

        var psi = new ProcessStartInfo
        {
            FileName = "cmd.exe",
            Arguments = $"/c timeout /t 2 & rmdir /s /q \"{installDir}\"",
            CreateNoWindow = true,
            UseShellExecute = false,
            WindowStyle = ProcessWindowStyle.Hidden
        };
        Process.Start(psi);
    }
}
```

---

### File 5: Entry Point & Uninstallation Dispatcher (`App.xaml.cs`)
```csharp
using System;
using System.Windows;

namespace AppName.Installer;

public partial class App : Application
{
    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // Check for uninstallation switch
        if (e.Args.Length > 0 && e.Args[0].Equals("/uninstall", StringComparison.OrdinalIgnoreCase))
        {
            var res = MessageBox.Show(
                "Are you sure you want to uninstall AppName?",
                "Uninstall AppName",
                MessageBoxButton.YesNo,
                MessageBoxImage.Question);

            if (res == MessageBoxResult.Yes)
            {
                InstallService.Uninstall();
                MessageBox.Show(
                    "AppName has been successfully uninstalled.",
                    "Uninstall Complete",
                    MessageBoxButton.OK,
                    MessageBoxImage.Information);
            }
            Shutdown();
            return;
        }

        // Default: Open Graphical Setup Wizard
        new InstallerWindow().Show();
    }
}
```

---

### File 6: Automated Packaging Script (`scripts\build_publish.ps1`)
```powershell
# ==============================================================================
# Automated Build & Packaging Pipeline
# ==============================================================================
$ErrorActionPreference = "Stop"

$PublishDir = "publish"
$AppStagingDir = "publish\app"
$InstallerStagingDir = "publish\installer_staging"

Write-Host "1. Cleaning previous publish directories..."
if (Test-Path $PublishDir) { Remove-Item $PublishDir -Recurse -Force }
New-Item -ItemType Directory -Path $AppStagingDir -Force | Out-Null

Write-Host "2. Publishing core application in self-contained single-file mode..."
dotnet publish src\AppName\AppName.csproj `
    -c Release `
    -r win-x64 `
    --self-contained `
    -p:PublishSingleFile=true `
    -p:IncludeNativeLibrariesForSelfExtract=true `
    -o $AppStagingDir

# Copy standalone binary directly to publish folder
Copy-Item "$AppStagingDir\AppName.exe" "$PublishDir\AppName.exe" -Force

Write-Host "3. Creating compressed Payload.zip archive..."
$PayloadZip = "src\Installer\Payload.zip"
if (Test-Path $PayloadZip) { Remove-Item $PayloadZip -Force }
Compress-Archive -Path "$AppStagingDir\*" -DestinationPath $PayloadZip -Force

Write-Host "4. Compiling self-contained Setup Installer executable..."
dotnet publish src\Installer\AppNameInstaller.csproj `
    -c Release `
    -r win-x64 `
    --self-contained `
    -p:PublishSingleFile=true `
    -o $InstallerStagingDir

Copy-Item "$InstallerStagingDir\AppNameSetup.exe" "$PublishDir\AppNameSetup.exe" -Force

Write-Host "=================================================="
Write-Host " BUILD & PACKAGING COMPLETED SUCCESSFULLY!        "
Write-Host "  Setup Installer: $PublishDir\AppNameSetup.exe    "
Write-Host "  Portable App:    $PublishDir\AppName.exe         "
Write-Host "=================================================="
```

---

## 3. Verification & Quality Checklist for Agents

Before completing the installation task, verify the following:
- [ ] **Standalone Execution**: `AppNameSetup.exe` runs on a clean Windows machine without needing prior .NET runtimes.
- [ ] **Zero UAC Requirement**: Installs smoothly under `%LOCALAPPDATA%\Programs\<AppName>`.
- [ ] **Process Protection**: Automatically detects and closes running instances in the target directory to avoid extraction errors.
- [ ] **Shortcut Creation**: Generates functional Desktop and Start Menu `.lnk` shortcuts with correct icon indices.
- [ ] **Add/Remove Programs Integration**: Registers under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall` with working display metadata and `/uninstall` command.
- [ ] **Clean Removal**: Uninstallation deletes shortcuts, registry keys, and directory contents without leaving orphaned files.
```
