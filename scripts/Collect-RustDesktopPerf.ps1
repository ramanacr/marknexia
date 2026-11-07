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
    [ValidateRange(1000, 120000)][int]$TimeoutMs = 30000,
    # The whole collection stops (with a diagnostic) before the CI step's own
    # 15-minute limit would kill it silently.
    [ValidateRange(1, 600)][int]$TotalTimeoutMinutes = 12
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
    [DllImport("user32.dll")] private static extern bool IsWindowEnabled(IntPtr window);
    [DllImport("user32.dll")] private static extern bool IsHungAppWindow(IntPtr window);
    [DllImport("user32.dll")] private static extern IntPtr GetWindow(IntPtr window, uint command);
    // Never sends a message, so a hung shell cannot block the dump.
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int InternalGetWindowText(IntPtr window, StringBuilder text, int capacity);
    public static string DescribeWindows(uint processId) {
        var lines = new StringBuilder();
        EnumWindows((window, _) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != processId) { return true; }
            var name = new StringBuilder(64);
            GetClassName(window, name, name.Capacity);
            var text = new StringBuilder(256);
            InternalGetWindowText(window, text, text.Capacity);
            lines.AppendFormat("  0x{0:X} class={1} title=\"{2}\" visible={3} enabled={4} hung={5} owner=0x{6:X}\n",
                window.ToInt64(), name, text, IsWindowVisible(window), IsWindowEnabled(window),
                IsHungAppWindow(window), GetWindow(window, 4).ToInt64());
            return true;
        }, IntPtr.Zero);
        return lines.Length == 0 ? "  (no top-level windows)\n" : lines.ToString();
    }
    // Process list from a Toolhelp snapshot: no WMI, so it cannot stall the
    // way Win32_Process queries can after many launches on a hosted runner.
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct ProcessEntry {
        public uint dwSize; public uint cntUsage; public uint th32ProcessID; public IntPtr th32DefaultHeapID;
        public uint th32ModuleID; public uint cntThreads; public uint th32ParentProcessID; public int pcPriClassBase;
        public uint dwFlags; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szExeFile;
    }
    [DllImport("kernel32.dll", SetLastError = true)] private static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint processId);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] private static extern bool Process32FirstW(IntPtr snapshot, ref ProcessEntry entry);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] private static extern bool Process32NextW(IntPtr snapshot, ref ProcessEntry entry);
    [DllImport("kernel32.dll")] private static extern bool CloseHandle(IntPtr handle);
    public static string[] Processes() {
        var result = new System.Collections.Generic.List<string>();
        IntPtr snapshot = CreateToolhelp32Snapshot(2, 0);
        if (snapshot == new IntPtr(-1)) { return result.ToArray(); }
        try {
            var entry = new ProcessEntry { dwSize = (uint)Marshal.SizeOf(typeof(ProcessEntry)) };
            for (bool more = Process32FirstW(snapshot, ref entry); more; more = Process32NextW(snapshot, ref entry)) {
                result.Add(entry.th32ProcessID + "|" + entry.th32ParentProcessID + "|" + entry.szExeFile);
            }
        }
        finally { CloseHandle(snapshot); }
        return result.ToArray();
    }
    private static System.Threading.Timer watchdog;
    // Ends the collector process if it has not finished in time. Pure .NET
    // timer callback: no PowerShell runspace is needed on the timer thread.
    public static void StartWatchdog(int minutes) {
        watchdog = new System.Threading.Timer(_ => {
            Console.Error.WriteLine("Collector watchdog: no completion within " + minutes + " minutes; a call blocked without a timeout.");
            Environment.Exit(3);
        }, null, TimeSpan.FromMinutes(minutes), System.Threading.Timeout.InfiniteTimeSpan);
    }
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
    $all = @([PerfWindowProbe]::Processes() | ForEach-Object {
            $fields = $_.Split('|', 3)
            [pscustomobject]@{ ProcessId = [int]$fields[0]; ParentProcessId = [int]$fields[1]; Name = $fields[2] }
        })
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

# What the shell process is doing: its state and its top-level windows (a
# modal dialog or a message box shows up here), so a stall is never silent.
function Get-ShellDiagnostic([Diagnostics.Process]$Process) {
    try {
        $Process.Refresh()
        $state = if ($Process.HasExited) { "exited with code $($Process.ExitCode)" }
        else { "running, responding=$($Process.Responding), threads=$($Process.Threads.Count)" }
        $windows = if ($Process.HasExited) { '' } else { [PerfWindowProbe]::DescribeWindows([uint32]$Process.Id) }
        return "shell $($Process.Id) $state`n$windows"
    }
    catch { return "shell diagnostic unavailable: $_" }
}

function Invoke-Run([string]$UserData) {
    $env:MARKNEXIA_WEBVIEW2_USER_DATA = $UserData
    # Default settings for every run: never the user's settings file.
    $env:MARKNEXIA_SETTINGS_PATH = Join-Path $UserData 'settings.json'
    # Own single-instance key: never hand off to another running shell.
    $env:MARKNEXIA_INSTANCE_KEY = "mnx-perf-$PID-$([Guid]::NewGuid().ToString('N'))"
    $clock = [Diagnostics.Stopwatch]::StartNew()
    # CreateProcess directly: no ShellExecuteEx (Start-Process), which depends
    # on the desktop shell and can wait without a timeout. The environment
    # variables above are inherited.
    $start = [Diagnostics.ProcessStartInfo]::new($exe)
    $start.UseShellExecute = $false
    if ($documentPath) { $start.ArgumentList.Add($documentPath) }
    $process = [Diagnostics.Process]::Start($start)
    $sample = [ordered]@{}
    $window = [IntPtr]::Zero
    try {
        $id = $process.Id
        while (-not $sample.Contains('firstRenderMs')) {
            if ($clock.ElapsedMilliseconds -gt $TimeoutMs) {
                throw "Run timed out; reached: $($sample.Keys -join ', ')`n$(Get-ShellDiagnostic $process)"
            }
            if ($process.HasExited) { throw "Shell exited early with code $($process.ExitCode)." }
            if (Test-EventSignaled "Local\Marknexia.StartupFailed.$id") {
                throw "The shell signalled StartupFailed (no startup document or placeholder can render); its status bar names the error."
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
        if (-not $process.WaitForExit(10000)) {
            Write-Host "The shell did not exit within 10 s of WM_CLOSE; ending it.`n$(Get-ShellDiagnostic $process)"
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            [void]$process.WaitForExit(10000)
        }
        foreach ($child in $children) {
            $live = Get-Process -Id $child.ProcessId -ErrorAction SilentlyContinue
            if ($live -and -not $live.WaitForExit(10000)) { Stop-Process -Id $child.ProcessId -Force -ErrorAction SilentlyContinue }
        }
        $process.Dispose()
        Remove-Item Env:\MARKNEXIA_WEBVIEW2_USER_DATA -ErrorAction SilentlyContinue
        Remove-Item Env:\MARKNEXIA_SETTINGS_PATH -ErrorAction SilentlyContinue
        Remove-Item Env:\MARKNEXIA_INSTANCE_KEY -ErrorAction SilentlyContinue
    }
}

# A launch can fail transiently on hosted runners (e.g. STATUS_DLL_INIT_FAILED
# 0xC0000142 when desktop resources run low). Retry such a run at most twice;
# every retry is recorded in the samples so nothing is hidden.
$script:retries = [System.Collections.Generic.List[object]]::new()
$script:collection = [Diagnostics.Stopwatch]::StartNew()
# Hard backstop: if any call blocks past the overall deadline, end the
# collector with a clear error instead of running into the CI step timeout.
[PerfWindowProbe]::StartWatchdog($TotalTimeoutMinutes + 1)
function Invoke-MeasuredRun([string]$UserData, [string]$Label) {
    if ($script:collection.Elapsed.TotalMinutes -gt $TotalTimeoutMinutes) {
        throw "Collection exceeded $TotalTimeoutMinutes minutes before run $Label."
    }
    for ($attempt = 1; ; $attempt++) {
        try {
            $run = [Diagnostics.Stopwatch]::StartNew()
            $sample = Invoke-Run $UserData
            Write-Host ("{0}: first render {1:N0} ms, run {2:N1} s" -f $Label, $sample.firstRenderMs, $run.Elapsed.TotalSeconds)
            return $sample
        }
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
