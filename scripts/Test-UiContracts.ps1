[CmdletBinding()]
param(
    [string]$Root = (Join-Path $PSScriptRoot "..")
)

$xaml = Get-Content -Raw (Join-Path $Root "src/Marknexia.App/MainWindow.xaml")
$appXaml = Get-Content -Raw (Join-Path $Root "src/Marknexia.App/App.xaml")
$windowCode = Get-Content -Raw (Join-Path $Root "src/Marknexia.App/MainWindow.xaml.cs")
$manifest = Get-Content -Raw (Join-Path $Root "src/Marknexia.App/Package.appxmanifest")
$setup = Get-Content -Raw (Join-Path $Root "src/Marknexia.Setup/Program.cs")
$setupXaml = Get-Content -Raw (Join-Path $Root "src/Marknexia.Setup/InstallerWindow.xaml")
$bridge = Get-Content -Raw (Join-Path $Root "src/Marknexia.Rendering/Assets/bridge.js")
$documentCss = Get-Content -Raw (Join-Path $Root "src/Marknexia.Rendering/Assets/github-markdown.css")
$welcomePrimary = [regex]::Match($xaml, '(?s)<Button x:Name="WelcomePrimaryAction".*?</Button>').Value

$checks = @(
    @{ Name = "window title bar icon"; Pass = $windowCode.Contains('SetIcon(') },
    @{ Name = "resizable sidebar handle"; Pass = $xaml.Contains('SidebarResizeHandle') -and $xaml.Contains('PointerPressed="SidebarResizeHandle_PointerPressed"') },
    @{ Name = "sidebar tabs"; Pass = $xaml.Contains('<TabView x:Name="SidebarModeSelector"') -and -not $xaml.Contains('<RadioButtons x:Name="SidebarModeSelector"') },
    @{ Name = "document map header row"; Pass = $xaml.Contains('x:Name="DocumentMapHeader"') -and $xaml.Contains('Grid.Row="2"') },
    @{ Name = "distinct markdown shell icon"; Pass = $manifest.Contains('MarkdownFileLogo') -and $setup.Contains('MarkdownFileLogo') },
    @{ Name = "preview handler on each extension"; Pass = $setup.Contains('extensionKey.CreateSubKey($"ShellEx\\{PreviewHandlerAssociation}")') },
    @{ Name = "Mermaid zoom and expand bridge"; Pass = $bridge.Contains('zoom-in') -and $bridge.Contains('pointerdown') -and $bridge.Contains('toggleDiagramExpanded') -and (Get-Content -Raw (Join-Path $Root 'src/Marknexia.Rendering/Assets/github-markdown.css')).Contains('overflow: auto') },
    @{ Name = "remote image setting is brokered"; Pass = $windowCode.Contains('AllowRemoteAssets') -and (Get-Content -Raw (Join-Path $Root 'src/Marknexia.App/MainWindow.Resources.cs')).Contains('AllowRemoteAssets') },
    @{ Name = "open shortcut tooltip is start-page scoped"; Pass = $windowCode.Contains('ToolTipService.SetToolTip(') -and $windowCode.Contains('OpenFileButton') -and $xaml.Contains('KeyboardAcceleratorPlacementMode="Hidden"') -and -not $xaml.Contains('ToolTipService.ToolTip="Open File (Ctrl+O)"') },
    @{ Name = "shell open command quotes file argument"; Pass = $setup.Contains('string fileArgument = Quote("%1")') -and $setup.Contains('command.SetValue(null, $"{Quote(executable)} {fileArgument}")') },
    @{ Name = "branded installer wizard"; Pass = $setupXaml.Contains('Text="Marknexia Setup"') -and $setupXaml.Contains('ConfigStepPanel') -and $setupXaml.Contains('ProgressStepPanel') -and $setupXaml.Contains('CompleteStepPanel') -and $setupXaml.Contains('assets/app.ico') }
    # Would fail if a System-theme transition left existing WebView surfaces on the old fallback color.
    @{ Name = "Metallic Radium dark theme"; Pass = $appXaml.Contains('Color="#171A1C"') -and $appXaml.Contains('Color="#B7FF3C"') -and $welcomePrimary.Contains('Color="#C9FF70"') -and $welcomePrimary.Contains('Color="#8FD622"') -and $appXaml.Contains('ButtonBorderBrushPointerOver') -and $appXaml.Contains('ButtonBorderBrushPressed') -and $appXaml.Contains('MarknexiaOnPrimaryBrush') -and $welcomePrimary.Contains('x:Key="ButtonForeground" Color="#172000"') -and $documentCss.Contains('--color-canvas-default: #171a1c') -and $windowCode.Contains('ThemeResolution.ResolveWebViewBackgroundArgb') -and $windowCode.Contains('ActualThemeChanged') }
    @{ Name = "Light semantic palette remains unchanged"; Pass = $appXaml.Contains('x:Key="MarknexiaHoverBrush" Color="#E2E8F0"') -and $appXaml.Contains('x:Key="MarknexiaDisabledTextBrush" Color="#94A3B8"') -and $appXaml.Contains('x:Key="MarknexiaSuccessBrush" Color="#1A7F37"') -and $appXaml.Contains('x:Key="MarknexiaInfoBrush" Color="#2563EB"') -and $appXaml.Contains('x:Key="MarknexiaWarningBrush" Color="#9A6700"') -and $appXaml.Contains('x:Key="MarknexiaErrorBrush" Color="#CF222E"') }
    # Would fail if radium green escaped the welcome primary action into neutral buttons.
    @{ Name = "Dark neutral buttons and scoped welcome primary action"; Pass = $appXaml.Contains('x:Key="ButtonBackground" Color="#303639"') -and $appXaml.Contains('x:Key="ButtonBackgroundPointerOver" Color="#383F42"') -and $appXaml.Contains('x:Key="ButtonBackgroundPressed" Color="#24292C"') -and $appXaml.Contains('x:Key="ButtonForeground" Color="#F1F4EF"') -and $appXaml.Contains('x:Key="ButtonBorderBrush" Color="#485054"') -and $welcomePrimary.Contains('x:Key="ButtonBackground" Color="#B7FF3C"') -and $welcomePrimary.Contains('x:Key="ButtonBackgroundPointerOver" Color="#C9FF70"') -and $welcomePrimary.Contains('x:Key="ButtonBackgroundPressed" Color="#8FD622"') -and $welcomePrimary.Contains('x:Key="ButtonForeground" Color="#172000"') }
)

$failed = @($checks | Where-Object { -not $_.Pass })
if ($failed.Count -gt 0) {
    throw "UI contract checks failed: $($failed.Name -join ', ')"
}

Write-Output "UI contract checks passed: $($checks.Count)"
