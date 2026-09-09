#requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$InstallerPath,
    [ValidateSet("x64", "arm64")]
    [string]$ExpectedArchitecture = "x64",
    [switch]$StaticOnly
)

$ErrorActionPreference = "Stop"
$installer = (Resolve-Path $InstallerPath -ErrorAction Stop).Path
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "marknexia-installer-test-$([Guid]::NewGuid().ToString('N'))"
$installDirectory = Join-Path $testRoot "install"
$scope = "Test-$([Guid]::NewGuid().ToString('N'))"
$startMenuShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Marknexia-$scope\Marknexia.lnk"
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Marknexia-$scope"
$programId = "Marknexia.MarkdownFile.$scope"

if ($StaticOnly) {
    $bytes = [IO.File]::ReadAllBytes($installer)
    if ($bytes.Length -lt 0x40 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
        throw "Installer is not a valid PE executable."
    }
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
    if ($peOffset -lt 0 -or $peOffset + 6 -gt $bytes.Length -or [Text.Encoding]::ASCII.GetString($bytes, $peOffset, 4) -ne "PE`0`0") {
        throw "Installer PE header is invalid."
    }
    $machine = [BitConverter]::ToUInt16($bytes, $peOffset + 4)
    $expectedMachine = if ($ExpectedArchitecture -eq "x64") { [uint16]0x8664 } else { [uint16]0xAA64 }
    if ($machine -ne $expectedMachine) {
        throw "Installer machine 0x$('{0:X4}' -f $machine) does not match $ExpectedArchitecture."
    }
    Write-Host "Installer static checks passed: valid PE and $ExpectedArchitecture machine type." -ForegroundColor Green
    return
}
$extensionKeys = @(".md", ".markdown", ".mdown", ".mkdn")

function Invoke-Installer([string[]]$Arguments) {
    $process = Start-Process -FilePath $installer -ArgumentList $Arguments -Wait -PassThru -WindowStyle Hidden
    if ($process.ExitCode -ne 0) {
        throw "Installer command failed with exit code $($process.ExitCode): $($Arguments -join ' ')"
    }
}

function Assert-EventuallyMissing([string]$Path) {
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        if (-not (Test-Path -LiteralPath $Path)) { return }
        Start-Sleep -Milliseconds 250
    }
    throw "Path was not removed after uninstall: $Path"
}

try {
    New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
    Invoke-Installer @("--verify", "--architecture", $ExpectedArchitecture)

    Invoke-Installer @("--install", "--silent", "--scope", $scope, "--dir", $installDirectory)
    foreach ($requiredFile in @("Marknexia.App.exe", "marknexia-sbom.spdx.json", "MarknexiaSetup.exe")) {
        if (-not (Test-Path (Join-Path $installDirectory $requiredFile) -PathType Leaf)) {
            throw "Installer did not deploy required file: $requiredFile"
        }
    }
    if (-not (Test-Path $startMenuShortcut -PathType Leaf)) {
        throw "Installer did not create the Start Menu shortcut: $startMenuShortcut"
    }
    if (-not (Test-Path $uninstallKey)) {
        throw "Installer did not register its uninstaller."
    }
    foreach ($extension in $extensionKeys) {
        $extensionPath = "Registry::HKEY_CURRENT_USER\Software\Classes\$extension"
        if (-not (Test-Path $extensionPath) -or (Get-ItemProperty -LiteralPath $extensionPath).'(default)' -ne $programId) {
            throw "Installer did not register the $extension file association."
        }
    }

    Invoke-Installer @("--uninstall", "--silent", "--scope", $scope, "--dir", $installDirectory)
    Assert-EventuallyMissing $installDirectory
    Assert-EventuallyMissing $startMenuShortcut
    if (Test-Path $uninstallKey) { throw "Installer uninstall registration remains after uninstall." }
    foreach ($extension in $extensionKeys) {
        if (Test-Path "Registry::HKEY_CURRENT_USER\Software\Classes\$extension") {
            throw "Installer file association remains after uninstall: $extension"
        }
    }

    Write-Host "Installer checks passed: embedded payload, $ExpectedArchitecture install, Start Menu, file associations, uninstall registration, and cleanup." -ForegroundColor Green
}
finally {
    if (Test-Path $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue }
}
