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

Scenarios: empty-shell starts with no document. small-document passes the
pinned fixture as the command-line argument; large-document generates its
file in %TEMP% exactly as the scenario describes, hashes it, and passes that.
FirstRender then means the opened document's navigation completed, and a run
fails fast when the shell signals Local\Marknexia.StartupFailed.<pid>. The
provenance fixtureDigest is the SHA-256 of the file actually opened, and it
must equal the artifact measurement's fixtureDigest. repository-scan is not
collectable yet (no repository sidebar).

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
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$exe = [IO.Path]::GetFullPath((Join-Path $ArtifactsRoot $artifact.nativeArtifact.path))
$exeHash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
$record = @($artifact.artifacts.files | Where-Object { $_.path -ceq $artifact.nativeArtifact.path })
if ($record.Count -ne 1 -or $record[0].sha256 -cne $exeHash) {
    throw 'Native artifact on disk does not match the artifact measurement record.'
}

function Get-Sha256Hex([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# The document the shell opens on its command line, and the digest of exactly
# those bytes. empty-shell opens nothing and keeps the artifact's digest.
$documentPath = $null
$generatedDocument = $null
switch ($scenarioData.id) {
    'empty-shell' { $fixtureDigest = $artifact.fixtureDigest }
    'small-document' {
        $documentPath = [IO.Path]::GetFullPath((Join-Path $repositoryRoot $scenarioData.fixture))
        if (-not (Test-Path -LiteralPath $documentPath -PathType Leaf)) { throw "Scenario fixture not found: $documentPath" }
        $fixtureDigest = Get-Sha256Hex $documentPath
    }
    'large-document' {
        $fixture = $scenarioData.fixture
        $source = [IO.Path]::GetFullPath((Join-Path $repositoryRoot $fixture.source))
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Scenario fixture source not found: $source" }
        $repeat = [int]$fixture.repeat
        if ($repeat -lt 1) { throw 'large-document fixture.repeat must be positive.' }
        # "Concatenate the UTF-8 source with one newline separator per repeat":
        # each repeat is the source bytes followed by one LF, written byte for
        # byte (no BOM, no newline translation).
        $sourceBytes = [IO.File]::ReadAllBytes($source)
        $unit = [byte[]]::new($sourceBytes.Length + 1)
        [Array]::Copy($sourceBytes, $unit, $sourceBytes.Length)
        $unit[$sourceBytes.Length] = 0x0A
        $generatedDocument = Join-Path ([IO.Path]::GetTempPath()) "marknexia-perf-large-document-$([Guid]::NewGuid().ToString('N')).md"
        $stream = [IO.File]::Open($generatedDocument, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
        try { foreach ($index in 1..$repeat) { $stream.Write($unit, 0, $unit.Length) } }
        finally { $stream.Dispose() }
        $documentPath = $generatedDocument
        # Hash the generated file before any capture run.
        $fixtureDigest = Get-Sha256Hex $documentPath
        Write-Host "Generated large-document fixture ($((Get-Item -LiteralPath $documentPath).Length) bytes, sha256 $fixtureDigest): $documentPath"
    }
    'repository-scan' {
        throw "Scenario 'repository-scan' needs a repository sidebar scan, which the Rust shell does not implement yet; collect empty-shell, small-document or large-document."
    }
    default { throw "Unknown scenario '$($scenarioData.id)'." }
}
if ($fixtureDigest -cne $artifact.fixtureDigest) {
    if ($generatedDocument) { Remove-Item -LiteralPath $generatedDocument -Force -ErrorAction SilentlyContinue }
    throw "Artifact measurement fixtureDigest '$($artifact.fixtureDigest)' does not match the '$($scenarioData.id)' fixture digest '$fixtureDigest'; record the artifact measurement with -FixtureDigest $fixtureDigest."
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
    $process = if ($documentPath) {
        Start-Process -FilePath $exe -ArgumentList "`"$documentPath`"" -PassThru
    }
    else {
        Start-Process -FilePath $exe -PassThru
    }
    $sample = [ordered]@{}
    $window = [IntPtr]::Zero
    try {
        $id = $process.Id
        while (-not $sample.Contains('firstRenderMs')) {
            if ($clock.ElapsedMilliseconds -gt $TimeoutMs) { throw "Run timed out; reached: $($sample.Keys -join ', ')" }
            if ($process.HasExited) { throw "Shell exited early with code $($process.ExitCode)." }
            if ($documentPath -and (Test-EventSignaled "Local\Marknexia.StartupFailed.$id")) {
                throw "The shell could not render the scenario document (StartupFailed); its status bar names the error."
            }
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

# A launch can fail transiently on hosted runners (e.g. STATUS_DLL_INIT_FAILED
# 0xC0000142 when desktop resources run low). Retry such a run at most twice;
# every retry is recorded in the samples so nothing is hidden.
$script:retries = [System.Collections.Generic.List[object]]::new()
function Invoke-MeasuredRun([string]$UserData, [string]$Label) {
    for ($attempt = 1; ; $attempt++) {
        try { return Invoke-Run $UserData }
        catch {
            if ($attempt -ge 3 -or "$_" -notmatch 'Shell exited early') { throw }
            $script:retries.Add([ordered]@{ run = $Label; attempt = $attempt; reason = "$_" })
            Start-Sleep -Seconds 2
        }
    }
}

$profileRoot = Join-Path ([IO.Path]::GetTempPath()) "marknexia-perf-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $profileRoot | Out-Null
try {
    $cold = foreach ($index in 1..$Runs) {
        $folder = Join-Path $profileRoot "cold-$index"
        Invoke-MeasuredRun $folder "cold-$index"
        Remove-Item -LiteralPath $folder -Recurse -Force -ErrorAction SilentlyContinue
    }
    $warmFolder = Join-Path $profileRoot 'warm'
    [void](Invoke-MeasuredRun $warmFolder 'warm-prime')
    $warm = foreach ($index in 1..$Runs) { Invoke-MeasuredRun $warmFolder "warm-$index" }
}
finally {
    Remove-Item -LiteralPath $profileRoot -Recurse -Force -ErrorAction SilentlyContinue
    if ($generatedDocument) { Remove-Item -LiteralPath $generatedDocument -Force -ErrorAction SilentlyContinue }
}

$samples = [ordered]@{
    sampleSource = 'collected-local'
    coldDefinition = 'fresh WebView2 user-data folder per run; OS file cache not flushed'
    warmDefinition = 'shared WebView2 user-data folder primed by one discarded run'
    webViewResidency = 'separate-process'
    retriedRuns = @($script:retries)
    provenance = [ordered]@{
        commit = $artifact.commit
        fixtureDigest = $fixtureDigest
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
