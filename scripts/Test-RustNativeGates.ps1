#requires -Version 7.0
<#
.SYNOPSIS
Runs the native (ignored) Rust feasibility gates on an interactive Windows host.

.DESCRIPTION
Runs the WebView2 environment and session native tests, the browser-process
exit recovery test (terminating only the WebView2 browser process that the
test itself started), the UI Automation smoke test against the built shell,
and the shell visibility contract. Intended for local evidence hosts and for
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

function Invoke-Gate([string]$Name, [scriptblock]$Body) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    try {
        & $Body
        if ($LASTEXITCODE -ne 0) { throw "exit code $LASTEXITCODE" }
        $results[$Name] = [ordered]@{ passed = $true; seconds = [math]::Round($clock.Elapsed.TotalSeconds, 1) }
    }
    catch {
        $results[$Name] = [ordered]@{ passed = $false; seconds = [math]::Round($clock.Elapsed.TotalSeconds, 1); error = "$_" }
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
$exe = Join-Path $root "target\$Target\release\marknexia-win32.exe"
Invoke-Gate 'uia-smoke' {
    $env:MARKNEXIA_SHELL_EXE = $exe
    cargo test --locked --target $Target -p marknexia-win32 --test automation_smoke -- --ignored --test-threads=1
}
Invoke-Gate 'shell-visible-contract' {
    pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Test-RustNativeShell.ps1') -ExePath $exe
}

$summary = [ordered]@{
    schemaVersion = 'rust-native-gates-v1'
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
