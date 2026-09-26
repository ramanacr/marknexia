#requires -Version 7.0
<#
.SYNOPSIS
Collects live desktop startup samples for the Rust shell on this machine.

.DESCRIPTION
Launches the native shell repeatedly and times three milestones from process
start: the visible top-level shell window, the shell's named
`Local\Marknexia.WebViewReady.<pid>` event (active WebView2 controller exists),
and `Local\Marknexia.FirstRender.<pid>` (active document navigation completed).
After first render it records host private bytes and the summed working set of
the WebView2 process tree separately, then closes the shell with WM_CLOSE.

Cold runs use a fresh WebView2 user-data folder per run (no profile cache).
Warm runs share one profile that is primed by a discarded run. Neither phase
flushes the OS file cache, so "cold" is a profile-cold approximation; the
output records that definition.

The output is a samples file for Measure-RustDesktopPerf.ps1, bound to the
supplied artifact measurement (commit, hardware, OS, native artifact SHA-256).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ArtifactMeasurementPath,
    [Parameter(Mandatory)][string]$ArtifactsRoot,
    [Parameter(Mandatory)][string]$Scenario,
    [Parameter(Mandatory)][string]$OutputPath,
    [ValidateRange(1, 1000)][int]$Runs = 30,
    [ValidateRange(1000, 120000)][int]$TimeoutMs = 30000
)

$ErrorActionPreference = 'Stop'
$artifact = Get-Content -LiteralPath $ArtifactMeasurementPath -Raw | ConvertFrom-Json -AsHashtable
$scenarioData = Get-Content -LiteralPath $Scenario -Raw | ConvertFrom-Json -AsHashtable
if ($scenarioData.id -ne 'empty-shell') {
    throw "Scenario '$($scenarioData.id)' needs document opening, which the shell does not implement yet; only empty-shell is collectable."
}
$exe = [IO.Path]::GetFullPath((Join-Path $ArtifactsRoot $artifact.nativeArtifact.path))
$exeHash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
$record = @($artifact.artifacts.files | Where-Object { $_.path -ceq $artifact.nativeArtifact.path })
if ($record.Count -ne 1 -or $record[0].sha256 -cne $exeHash) {
    throw 'Native artifact on disk does not match the artifact measurement record.'
}

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class PerfWindowProbe {
    private delegate bool EnumWindowsProc(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr data);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(IntPtr window, StringBuilder name, int capacity);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
    public static IntPtr FindVisibleShell(uint processId) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, _) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != processId || !IsWindowVisible(window)) { return true; }
            var name = new StringBuilder(64);
            GetClassName(window, name, name.Capacity);
            if (name.ToString() == "MarknexiaRustWindow") { found = window; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@

function Get-DescendantProcesses([int]$RootId) {
    $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name)
    $result = [System.Collections.Generic.List[object]]::new()
    $frontier = [System.Collections.Generic.Queue[int]]::new()
    $frontier.Enqueue($RootId)
    while ($frontier.Count -gt 0) {
        $parent = $frontier.Dequeue()
        foreach ($child in $all | Where-Object { $_.ParentProcessId -eq $parent }) {
            $result.Add($child)
            $frontier.Enqueue([int]$child.ProcessId)
        }
    }
    return $result
}

function Test-EventSignaled([string]$Name) {
    $handle = $null
    if (-not [System.Threading.EventWaitHandle]::TryOpenExisting($Name, [ref]$handle)) { return $false }
    try { return $handle.WaitOne(0) } finally { $handle.Dispose() }
}

function Invoke-Run([string]$UserData) {
    $env:MARKNEXIA_WEBVIEW2_USER_DATA = $UserData
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $exe -PassThru
    $sample = [ordered]@{}
    $window = [IntPtr]::Zero
    try {
        $id = $process.Id
        while (-not $sample.Contains('firstRenderMs')) {
            if ($clock.ElapsedMilliseconds -gt $TimeoutMs) { throw "Run timed out; reached: $($sample.Keys -join ', ')" }
            if ($process.HasExited) { throw "Shell exited early with code $($process.ExitCode)." }
            if (-not $sample.Contains('shellVisibleMs')) {
                $window = [PerfWindowProbe]::FindVisibleShell([uint32]$id)
                if ($window -ne [IntPtr]::Zero) { $sample.shellVisibleMs = [double]$clock.Elapsed.TotalMilliseconds }
            }
            if (-not $sample.Contains('webViewReadyMs') -and (Test-EventSignaled "Local\Marknexia.WebViewReady.$id")) {
                $sample.webViewReadyMs = [double]$clock.Elapsed.TotalMilliseconds
            }
            if ($sample.Contains('webViewReadyMs') -and (Test-EventSignaled "Local\Marknexia.FirstRender.$id")) {
                $sample.firstRenderMs = [double]$clock.Elapsed.TotalMilliseconds
            }
            Start-Sleep -Milliseconds 2
        }
        # Let post-render work settle before sampling memory.
        Start-Sleep -Milliseconds 1500
        $process.Refresh()
        $sample.hostPrivateBytes = [double]$process.PrivateMemorySize64
        $webViewSet = 0.0
        foreach ($child in Get-DescendantProcesses $id | Where-Object { $_.Name -ieq 'msedgewebview2.exe' }) {
            $live = Get-Process -Id $child.ProcessId -ErrorAction SilentlyContinue
            if ($live) { $webViewSet += [double]$live.WorkingSet64 }
        }
        if ($webViewSet -le 0) { throw 'No WebView2 process tree found under the shell.' }
        $sample.webView2WorkingSetBytes = $webViewSet
        return $sample
    }
    finally {
        $children = @(Get-DescendantProcesses $process.Id)
        if ($window -ne [IntPtr]::Zero) { [void][PerfWindowProbe]::PostMessage($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
        if (-not $process.WaitForExit(10000)) { Stop-Process -Id $process.Id -Force }
        foreach ($child in $children) {
            $live = Get-Process -Id $child.ProcessId -ErrorAction SilentlyContinue
            if ($live -and -not $live.WaitForExit(10000)) { Stop-Process -Id $child.ProcessId -Force -ErrorAction SilentlyContinue }
        }
        $process.Dispose()
        Remove-Item Env:\MARKNEXIA_WEBVIEW2_USER_DATA -ErrorAction SilentlyContinue
    }
}

$profileRoot = Join-Path ([IO.Path]::GetTempPath()) "marknexia-perf-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $profileRoot | Out-Null
try {
    $cold = foreach ($index in 1..$Runs) {
        $folder = Join-Path $profileRoot "cold-$index"
        Invoke-Run $folder
        Remove-Item -LiteralPath $folder -Recurse -Force -ErrorAction SilentlyContinue
    }
    $warmFolder = Join-Path $profileRoot 'warm'
    [void](Invoke-Run $warmFolder)
    $warm = foreach ($index in 1..$Runs) { Invoke-Run $warmFolder }
}
finally {
    Remove-Item -LiteralPath $profileRoot -Recurse -Force -ErrorAction SilentlyContinue
}

$samples = [ordered]@{
    sampleSource = 'collected-local'
    coldDefinition = 'fresh WebView2 user-data folder per run; OS file cache not flushed'
    warmDefinition = 'shared WebView2 user-data folder primed by one discarded run'
    webViewResidency = 'separate-process'
    provenance = [ordered]@{
        commit = $artifact.commit
        fixtureDigest = $artifact.fixtureDigest
        scenario = [ordered]@{
            id = $scenarioData.id
            sha256 = (Get-FileHash -LiteralPath $Scenario -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        architecture = $artifact.architecture
        nativeArtifact = $artifact.nativeArtifact
        nativeArtifactSha256 = $exeHash
        hardware = $artifact.hardware
        operatingSystem = $artifact.operatingSystem
    }
    cold = @($cold)
    warm = @($warm)
}
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($OutputPath))) | Out-Null
$samples | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $OutputPath -Encoding utf8NoBOM
Write-Host "Collected $Runs cold and $Runs warm samples to $OutputPath."
