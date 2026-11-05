#requires -Version 7.0
<#
.SYNOPSIS
Runs the native (ignored) Rust feasibility gates on an interactive Windows host.

.DESCRIPTION
Runs the WebView2 environment and session native tests, the browser-process
exit recovery test (terminating only the WebView2 browser process that the
test itself started), the UI Automation smoke test against the built shell,
the native accessibility gate (DPI awareness and target size, UIA structure
events, and the high-contrast palette, read only), and the shell visibility
contract. Intended for local evidence hosts and for
CI runners with an interactive desktop, on x64 and native ARM64.

Writes a JSON summary to -OutputPath. Exits non-zero if any gate fails.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Target,
    [Parameter(Mandatory)][string]$OutputPath
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$results = [ordered]@{}

function Invoke-Gate([string]$Name, [scriptblock]$Body, [int]$Attempts = 1) {
    # Desktop-input gates may be retried: hosted runners intermittently refuse
    # the foreground to a background process. Every failed attempt is kept in
    # the summary, so a retry never hides a failure.
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $failures = @()
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        try {
            & $Body
            if ($LASTEXITCODE -ne 0) { throw "exit code $LASTEXITCODE" }
            $results[$Name] = [ordered]@{ passed = $true; seconds = [math]::Round($clock.Elapsed.TotalSeconds, 1); attempts = $attempt; failedAttempts = $failures }
            return
        }
        catch {
            $failures += "attempt ${attempt}: $_"
        }
    }
    $results[$Name] = [ordered]@{ passed = $false; seconds = [math]::Round($clock.Elapsed.TotalSeconds, 1); attempts = $Attempts; failedAttempts = $failures; error = $failures[-1] }
}

# The hosted windows-11-arm image can raise a "Microsoft account" sign-in
# prompt (a system CoreWindow) at any time; it takes the foreground, and
# Windows then refuses to activate the shell under test (run 42106410645).
# On GitHub Actions only, a watcher closes that prompt's process while the
# gates run and logs each close. The ephemeral runner never signs in.
$promptWatcher = $null
if ($env:GITHUB_ACTIONS -eq 'true') {
    $promptWatcher = Start-ThreadJob {
        while ($true) {
            foreach ($prompt in Get-Process | Where-Object { $_.MainWindowTitle -eq 'Microsoft account' }) {
                "closed sign-in prompt: $($prompt.Id) $($prompt.ProcessName) $($prompt.Path)"
                Stop-Process -Id $prompt.Id -Force -ErrorAction SilentlyContinue
            }
            Start-Sleep -Milliseconds 500
        }
    }
}

Invoke-Gate 'webview-environment' {
    cargo test --locked --target $Target -p marknexia-webview --test environment -- --ignored --test-threads=1
}
Invoke-Gate 'webview-session' {
    cargo test --locked --target $Target -p marknexia-webview --lib -- --ignored --test-threads=1 --skip native_browser_exit
}

Invoke-Gate 'webview-browser-exit-recovery' {
    $log = Join-Path ([IO.Path]::GetTempPath()) "marknexia-browser-exit-$PID.log"
    $test = Start-Process -FilePath cargo -NoNewWindow -PassThru -RedirectStandardOutput $log -RedirectStandardError "$log.err" -ArgumentList @(
        'test', '--locked', '--target', $Target, '-p', 'marknexia-webview', '--lib', '--', '--ignored', '--exact',
        'session::native_tests::native_browser_exit_recreates_environment_and_immutable_tabs', '--test-threads=1')
    $deadline = (Get-Date).AddMinutes(10)
    $killed = $false
    while (-not $test.HasExited -and -not $killed -and (Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        $host_ = Get-CimInstance Win32_Process -Filter "Name LIKE 'marknexia_webview-%.exe'" | Select-Object -First 1
        if (-not $host_) { continue }
        $browser = Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe' AND ParentProcessId = $($host_.ProcessId)" |
            Where-Object { $_.CommandLine -notmatch '--type=' } | Select-Object -First 1
        if (-not $browser) { continue }
        Start-Sleep -Seconds 8
        Stop-Process -Id $browser.ProcessId -Force
        $killed = $true
    }
    $test.WaitForExit(600000) | Out-Null
    Get-Content $log | Select-String 'test result'
    if (-not $killed) { throw 'browser process was never observed' }
    if ($test.ExitCode -ne 0) { throw "browser-exit test failed ($($test.ExitCode))" }
    $global:LASTEXITCODE = 0
}

Invoke-Gate 'shell-build' { cargo build --locked --release --target $Target -p marknexia-win32 }
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$exe = Join-Path $targetDir "$Target\release\marknexia-win32.exe"
Invoke-Gate 'uia-smoke' {
    $env:MARKNEXIA_SHELL_EXE = $exe
    cargo test --locked --target $Target -p marknexia-win32 --test automation_smoke -- --ignored --test-threads=1
} -Attempts 3
# DPI awareness and 44-DIP tab targets, UIA structure events on tab close,
# and the palette against the current (read-only) high-contrast state.
Invoke-Gate 'uia-native-accessibility' {
    $env:MARKNEXIA_SHELL_EXE = $exe
    cargo test --locked --target $Target -p marknexia-win32 --test native_accessibility -- --ignored --test-threads=1 --nocapture
} -Attempts 3
Invoke-Gate 'shell-visible-contract' {
    pwsh -NoProfile -NonInteractive -File (Join-Path $PSScriptRoot 'Test-RustNativeShell.ps1') -ExePath $exe
}

$closedPrompts = @()
if ($promptWatcher) {
    Stop-Job $promptWatcher
    $closedPrompts = @(Receive-Job $promptWatcher)
    Remove-Job $promptWatcher
    $closedPrompts | ForEach-Object { $_ }
}

$summary = [ordered]@{
    schemaVersion = 'rust-native-gates-v1'
    closedSignInPrompts = $closedPrompts
    target = $Target
    commit = (git rev-parse HEAD).Trim()
    operatingSystem = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption, BuildNumber, OSArchitecture)
    processor = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()
    webView2Runtime = (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue).pv
    gates = $results
}
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($OutputPath))) | Out-Null
$summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $OutputPath -Encoding utf8NoBOM
$results.GetEnumerator() | ForEach-Object { "{0}: {1}" -f $_.Key, ($(if ($_.Value.passed) { 'PASS' } else { "FAIL $($_.Value.error)" })) }
if ($results.Values | Where-Object { -not $_.passed }) { exit 1 }
