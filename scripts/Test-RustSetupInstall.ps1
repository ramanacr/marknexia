#requires -Version 7.0
<#
.SYNOPSIS
CI only, on an ephemeral GitHub-hosted runner: installs the unsigned preview setup silently,
checks the files, registry and shortcut, upgrades it in place, uninstalls it through its
registered QuietUninstallString, and checks that everything was removed.

.DESCRIPTION
Refuses to run outside GitHub Actions: it writes HKCU keys and runs the setup executable.
The runner's architecture must match the setup's (x64 on windows-latest). The shell itself
is never started. Writes a JSON evidence file.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$SetupPath,
    [Parameter(Mandatory)][string]$PayloadDir,
    [Parameter(Mandatory)][string]$ExpectedVersion,
    [Parameter(Mandatory)][string]$OutputPath
)

$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -eq 'self-hosted') {
    throw 'Test-RustSetupInstall.ps1 runs only on ephemeral GitHub-hosted runners.'
}

$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Marknexia Rust Preview'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\MarknexiaRustPreview'
$progIdKey = 'HKCU:\Software\Classes\Marknexia.RustPreview.MarkdownFile'
$applicationKey = 'HKCU:\Software\Classes\Applications\marknexia-win32.exe'
$capabilitiesKey = 'HKCU:\Software\Marknexia\RustPreview'
$registeredApplications = 'HKCU:\Software\RegisteredApplications'
$programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
$shortcut = Join-Path $programs 'Marknexia (Rust preview).lnk'
$extensions = @('.md', '.markdown', '.mdown', '.mkdn')
$log = Join-Path $env:RUNNER_TEMP 'marknexia-rust-setup.log'
$checks = [Collections.Generic.List[string]]::new()

function Get-Digest([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $checks.Add($Message)
}
function Invoke-Setup([string]$Path, [string[]]$Arguments) {
    $quoted = @($Arguments | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } })
    $process = Start-Process -FilePath $Path -ArgumentList $quoted -Wait -PassThru -NoNewWindow
    $process.ExitCode
}
function Get-DefaultValue([string]$Key) {
    if (-not (Test-Path -LiteralPath $Key)) { return $null }
    (Get-Item -LiteralPath $Key).GetValue('')
}
function Test-OpenWith([string]$Extension) {
    $key = "HKCU:\Software\Classes\$Extension\OpenWithProgids"
    (Test-Path -LiteralPath $key) -and ((Get-Item -LiteralPath $key).GetValueNames() -contains 'Marknexia.RustPreview.MarkdownFile')
}

# The runner starts clean; nothing of the preview may exist.
Assert-That (-not (Test-Path -LiteralPath $installDir)) 'install folder absent before install'
Assert-That (-not (Test-Path -LiteralPath $uninstallKey)) 'uninstall key absent before install'
$defaultsBefore = @{}
foreach ($extension in $extensions) { $defaultsBefore[$extension] = Get-DefaultValue "HKCU:\Software\Classes\$extension" }

# --verify writes only its report.
$report = Join-Path $env:RUNNER_TEMP 'marknexia-rust-setup-verify.json'
Assert-That ((Invoke-Setup $SetupPath @('--verify', '--report', $report, '--log', $log)) -eq 0) '--verify exits 0'
$verify = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
Assert-That ($verify.version -ceq $ExpectedVersion -and @($verify.files).Count -eq 5) '--verify reports the version and five files'
foreach ($file in $verify.files) {
    Assert-That ($file.sha256 -ceq (Get-Digest (Join-Path $PayloadDir $file.name))) "--verify hash of $($file.name) matches the payload"
}
Assert-That (-not (Test-Path -LiteralPath $installDir)) '--verify installs nothing'

# Silent install with the opt-in associations.
Assert-That ((Invoke-Setup $SetupPath @('--install', '--silent', '--associate', '--log', $log)) -eq 0) 'silent install exits 0'
$expected = @((Get-ChildItem -LiteralPath $PayloadDir -File | ForEach-Object Name) + 'marknexia-rust-preview-setup.exe' | Sort-Object)
$actual = @(Get-ChildItem -LiteralPath $installDir -Force | ForEach-Object Name | Sort-Object)
Assert-That (($actual -join '|') -ceq ($expected -join '|')) "installed files are exactly the payload and the setup ($($actual -join ', '))"
foreach ($file in Get-ChildItem -LiteralPath $PayloadDir -File) {
    Assert-That ((Get-Digest (Join-Path $installDir $file.Name)) -ceq (Get-Digest $file.FullName)) "installed $($file.Name) matches the payload"
}
Assert-That ((Get-Digest (Join-Path $installDir 'marknexia-rust-preview-setup.exe')) -ceq (Get-Digest $SetupPath)) 'the installed uninstaller is this setup'
$entry = Get-ItemProperty -LiteralPath $uninstallKey
Assert-That ($entry.DisplayName -ceq 'Marknexia (Rust preview)' -and $entry.DisplayVersion -ceq $ExpectedVersion) 'uninstall entry name and version'
Assert-That ($entry.InstallLocation -ieq $installDir) 'uninstall entry location'
Assert-That ($entry.QuietUninstallString -ceq "`"$installDir\marknexia-rust-preview-setup.exe`" --uninstall --silent") 'quiet uninstall string'
Assert-That ($entry.NoModify -eq 1 -and $entry.NoRepair -eq 1 -and $entry.EstimatedSize -gt 0) 'uninstall entry flags and size'
Assert-That (-not (Test-Path -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Marknexia')) 'no .NET uninstall key was written'
$link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
Assert-That ((Test-Path -LiteralPath $shortcut) -and $link.TargetPath -ieq (Join-Path $installDir 'marknexia-win32.exe')) 'Start menu shortcut targets the installed shell'
Assert-That ((Get-DefaultValue "$progIdKey\shell\open\command") -ceq "`"$installDir\marknexia-win32.exe`" `"%1`"") 'ProgID open command'
foreach ($extension in $extensions) {
    Assert-That (Test-OpenWith $extension) "$extension lists the preview in OpenWithProgids"
    Assert-That ((Get-DefaultValue "HKCU:\Software\Classes\$extension") -ceq $defaultsBefore[$extension]) "$extension default value unchanged"
}
Assert-That ((Get-ItemProperty -LiteralPath $registeredApplications).MarknexiaRustPreview -ceq 'Software\Marknexia\RustPreview\Capabilities') 'Default Apps registration'

# Upgrade in place keeps the earlier opt-in.
Assert-That ((Invoke-Setup $SetupPath @('--install', '--silent', '--log', $log)) -eq 0) 'silent reinstall exits 0'
Assert-That ((Test-Path -LiteralPath $progIdKey) -and (Test-OpenWith '.md')) 'reinstall keeps the associations'
$leftovers = @(Get-ChildItem -LiteralPath (Split-Path -Parent $installDir) -Directory | Where-Object Name -like 'Marknexia Rust Preview.*')
Assert-That ($leftovers.Count -eq 0) 'no staging or backup folder remains'

# Uninstall through the registered quiet uninstall string.
$quiet = (Get-ItemProperty -LiteralPath $uninstallKey).QuietUninstallString
if ($quiet -cnotmatch '^"([^"]+)" (.+)$') { throw "Unexpected QuietUninstallString: $quiet" }
$uninstaller, $arguments = $Matches[1], @($Matches[2] -split ' ')
Assert-That ((Invoke-Setup $uninstaller (@($arguments) + @('--log', $log))) -eq 0) 'silent uninstall exits 0'
Assert-That (-not (Test-Path -LiteralPath $installDir)) 'install folder removed'
Assert-That (-not (Test-Path -LiteralPath $uninstallKey)) 'uninstall key removed'
Assert-That (-not (Test-Path -LiteralPath $shortcut)) 'shortcut removed'
Assert-That (-not (Test-Path -LiteralPath $progIdKey)) 'ProgID removed'
Assert-That (-not (Test-Path -LiteralPath $applicationKey)) 'Applications entry removed'
Assert-That (-not (Test-Path -LiteralPath $capabilitiesKey)) 'capabilities removed'
Assert-That ($null -eq (Get-ItemProperty -LiteralPath $registeredApplications -ErrorAction SilentlyContinue).MarknexiaRustPreview) 'Default Apps registration removed'
foreach ($extension in $extensions) {
    Assert-That (-not (Test-OpenWith $extension)) "$extension OpenWithProgids entry removed"
    Assert-That ((Get-DefaultValue "HKCU:\Software\Classes\$extension") -ceq $defaultsBefore[$extension]) "$extension default value unchanged after uninstall"
}
$moved = @(Get-ChildItem -LiteralPath ([IO.Path]::GetTempPath()) -File -Filter 'marknexia-rust-preview-uninstall-*.exe')
Assert-That ($moved.Count -eq 1) 'the running uninstaller moved itself to the temp folder'
$moved | Remove-Item -Force

[ordered]@{
    schemaVersion = 'rust-setup-install-round-trip-v1'
    setupSha256 = Get-Digest $SetupPath
    version = $ExpectedVersion
    checks = @($checks)
    log = @(Get-Content -LiteralPath $log)
} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $OutputPath -Encoding utf8NoBOM
Write-Output "Setup install round trip passed ($($checks.Count) checks)."
