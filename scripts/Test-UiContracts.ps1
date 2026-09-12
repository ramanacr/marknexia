[CmdletBinding()]
param(
    [string]$Root = (Join-Path $PSScriptRoot "..")
)

$xaml = Get-Content -Raw (Join-Path $Root "src/Marknexia.App/MainWindow.xaml")
$windowCode = Get-Content -Raw (Join-Path $Root "src/Marknexia.App/MainWindow.xaml.cs")
$manifest = Get-Content -Raw (Join-Path $Root "src/Marknexia.App/Package.appxmanifest")
$setup = Get-Content -Raw (Join-Path $Root "src/Marknexia.Setup/Program.cs")
$setupXaml = Get-Content -Raw (Join-Path $Root "src/Marknexia.Setup/InstallerWindow.xaml")
$bridge = Get-Content -Raw (Join-Path $Root "src/Marknexia.Rendering/Assets/bridge.js")

$checks = @(
    @{ Name = "window title bar icon"; Pass = $windowCode.Contains('SetIcon(') },
    @{ Name = "resizable sidebar handle"; Pass = $xaml.Contains('SidebarResizeHandle') -and $xaml.Contains('PointerPressed="SidebarResizeHandle_PointerPressed"') },
    @{ Name = "sidebar tabs"; Pass = $xaml.Contains('<TabView x:Name="SidebarModeSelector"') -and -not $xaml.Contains('<RadioButtons x:Name="SidebarModeSelector"') },
    @{ Name = "document map header row"; Pass = $xaml.Contains('x:Name="DocumentMapHeader"') -and $xaml.Contains('Grid.Row="2"') },
    @{ Name = "distinct markdown shell icon"; Pass = $manifest.Contains('MarkdownFileLogo') -and $setup.Contains('MarkdownFileLogo') },
    @{ Name = "Mermaid zoom and expand bridge"; Pass = $bridge.Contains('zoom-in') -and $bridge.Contains('pointerdown') -and $bridge.Contains('toggleDiagramExpanded') -and (Get-Content -Raw (Join-Path $Root 'src/Marknexia.Rendering/Assets/github-markdown.css')).Contains('overflow: auto') },
    @{ Name = "remote image setting is brokered"; Pass = $windowCode.Contains('AllowRemoteAssets') -and (Get-Content -Raw (Join-Path $Root 'src/Marknexia.App/MainWindow.Resources.cs')).Contains('AllowRemoteAssets') },
    @{ Name = "open shortcut tooltip is start-page scoped"; Pass = $windowCode.Contains('ToolTipService.SetToolTip(') -and $windowCode.Contains('OpenFileButton') -and -not $xaml.Contains('ToolTipService.ToolTip="Open File (Ctrl+O)"') },
    @{ Name = "branded installer wizard"; Pass = $setupXaml.Contains('Text="Marknexia Setup"') -and $setupXaml.Contains('ConfigStepPanel') -and $setupXaml.Contains('ProgressStepPanel') -and $setupXaml.Contains('CompleteStepPanel') -and $setupXaml.Contains('assets/app.ico') }
)

$failed = @($checks | Where-Object { -not $_.Pass })
if ($failed.Count -gt 0) {
    throw "UI contract checks failed: $($failed.Name -join ', ')"
}

Write-Output "UI contract checks passed: $($checks.Count)"
