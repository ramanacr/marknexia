#requires -Version 7.0

[CmdletBinding()]
param(
    [string]$PackagePath = "",
    [string]$StagingDirectory = "",
    [ValidateSet("x64", "arm64")]
    [string]$ExpectedArchitecture = "x64"
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if ([string]::IsNullOrWhiteSpace($PackagePath)) {
    $PackagePath = Join-Path $repositoryRoot "store-submission\Marknexia-Store-Ready.msix"
}
if ([string]::IsNullOrWhiteSpace($StagingDirectory)) {
    $StagingDirectory = Join-Path $repositoryRoot "store-submission\msix-staging"
}

$PackagePath = (Resolve-Path $PackagePath -ErrorAction Stop).Path
$StagingDirectory = (Resolve-Path $StagingDirectory -ErrorAction Stop).Path

if ((Get-Item $PackagePath).Length -le 0) {
    throw "MSIX package is empty: $PackagePath"
}

$manifestPath = Join-Path $StagingDirectory "AppxManifest.xml"
if (-not (Test-Path $manifestPath -PathType Leaf)) {
    throw "MSIX staging manifest is missing: $manifestPath"
}

[xml]$manifest = Get-Content $manifestPath -Raw
$identity = $manifest.SelectSingleNode("//*[local-name()='Identity']")
$application = $manifest.SelectSingleNode("//*[local-name()='Application']")
if ($null -eq $identity -or $identity.GetAttribute("Name") -ne "RRCLabs.Marknexia") {
    throw "Unexpected package identity. Expected RRCLabs.Marknexia."
}
if ($identity.ProcessorArchitecture -ne $ExpectedArchitecture -or $identity.Version -notmatch '^\d+\.\d+\.\d+\.\d+$') {
    throw "Package identity must contain a numeric $ExpectedArchitecture version."
}
if ($null -eq $application -or $application.Executable -ne "Marknexia.App.exe") {
    throw "Package application entry point is invalid."
}

foreach ($requiredFile in @(
        "Marknexia.App.exe",
        "Marknexia.App.dll",
        "Marknexia.App.pri",
        "marknexia-sbom.spdx.json",
        "Assets\StoreLogo.png",
        "Assets\Square44x44Logo.png")) {
    if (-not (Test-Path (Join-Path $StagingDirectory $requiredFile) -PathType Leaf)) {
        throw "MSIX staging is missing required file: $requiredFile"
    }
}

$makeAppx = (Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Recurse -Filter "makeappx.exe" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "x64" } |
    Select-Object -First 1).FullName
if ([string]::IsNullOrWhiteSpace($makeAppx) -or -not (Test-Path $makeAppx)) {
    throw "makeappx.exe was not found in the installed Windows SDK."
}

$unpackDirectory = Join-Path ([IO.Path]::GetTempPath()) "marknexia-msix-verify-$([Guid]::NewGuid().ToString('N'))"
try {
    New-Item -ItemType Directory -Path $unpackDirectory -Force | Out-Null
    & $makeAppx unpack /p $PackagePath /d $unpackDirectory | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "makeappx failed to unpack the MSIX (exit code $LASTEXITCODE)."
    }

    foreach ($requiredFile in @("Marknexia.App.exe", "Marknexia.App.pri", "marknexia-sbom.spdx.json")) {
        if (-not (Test-Path (Join-Path $unpackDirectory $requiredFile) -PathType Leaf)) {
            throw "Unpacked MSIX is missing required file: $requiredFile"
        }
    }
}
finally {
    if (Test-Path $unpackDirectory) {
        Remove-Item -LiteralPath $unpackDirectory -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "MSIX package checks passed: identity $($identity.GetAttribute('Name')), $ExpectedArchitecture version $($identity.GetAttribute('Version')), required payloads, SBOM, and MakeAppx unpackability." -ForegroundColor Green
