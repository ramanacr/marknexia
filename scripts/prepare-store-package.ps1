param(
    [string]$PackageName = "RRCLabs.Marknexia",
    [string]$PublisherId = "CN=4BC6AFD6-49C8-46B2-A096-9AF3F2B78CB8",
    [string]$PublisherDisplayName = "RRC Labs",
    [string]$Version = "",
    [ValidateSet("x64", "ARM64")]
    [string]$Platform = "x64"
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if ([string]::IsNullOrWhiteSpace($Version)) {
    $propsPath = Join-Path $repositoryRoot "Directory.Build.props"
    $props = Get-Content $propsPath -Raw
    $major = [regex]::Match($props, '<VersionMajor[^>]*>(\d+)</VersionMajor>').Groups[1].Value
    $minor = [regex]::Match($props, '<VersionMinor[^>]*>(\d+)</VersionMinor>').Groups[1].Value
    $build = (& git -C $repositoryRoot rev-list --count HEAD).Trim()
    if ([string]::IsNullOrWhiteSpace($major) -or [string]::IsNullOrWhiteSpace($minor) -or $build -notmatch '^\d+$') {
        throw "Could not derive a numeric package version. Supply -Version Major.Minor.Build.0."
    }
    $Version = "$major.$minor.$build.0"
}

if ($Version -notmatch '^\d+\.\d+\.\d+\.\d+$') {
    throw "Package version '$Version' must have four numeric components, for example 1.0.11.0."
}

$architecture = if ($Platform -eq "ARM64") { "arm64" } else { "x64" }
$publishDir = Join-Path $PSScriptRoot "..\src\Marknexia.App\bin\$Platform\Release\net10.0-windows10.0.19041.0"
if (-not (Test-Path (Join-Path $publishDir "Marknexia.App.exe"))) {
    $publishDir = Join-Path $PSScriptRoot "..\src\Marknexia.App\bin\Release\net10.0-windows10.0.19041.0"
    $runtimeOutput = Join-Path $publishDir ("win-" + $architecture)
    if (Test-Path (Join-Path $runtimeOutput "Marknexia.App.exe")) {
        $publishDir = $runtimeOutput
    }
}
if (-not (Test-Path (Join-Path $publishDir "Marknexia.App.exe"))) {
    throw "Release output was not found. Build the app with scripts/Build-App.ps1 first."
}
foreach ($requiredFile in @("Marknexia.App.dll", "Marknexia.App.pri", "marknexia-sbom.spdx.json")) {
    if (-not (Test-Path (Join-Path $publishDir $requiredFile))) {
        throw "Release output is missing required package file: $requiredFile"
    }
}
$manifestPath = Join-Path $PSScriptRoot "..\src\Marknexia.App\Package.appxmanifest"
$outputDir = Join-Path $PSScriptRoot "..\store-submission"
$platformSuffix = if ($Platform -eq "x64") { "" } else { "-$Platform" }
$msixStaging = Join-Path $outputDir ("msix-staging" + $platformSuffix)
$outputMsix = Join-Path $outputDir ("Marknexia-Store-Ready" + $platformSuffix + ".msix")

Write-Host "Preparing Microsoft Store package for $Platform with Publisher: $PublisherId" -ForegroundColor Cyan

# Create staging directory
if (Test-Path $msixStaging) { Remove-Item $msixStaging -Recurse -Force }
New-Item -ItemType Directory -Force -Path $msixStaging | Out-Null
Copy-Item "$publishDir\*" -Destination "$msixStaging\" -Recurse
Get-ChildItem -Path $msixStaging -Recurse -Include "*.pdb" | Remove-Item -Force -ErrorAction SilentlyContinue
if (Test-Path (Join-Path $msixStaging "win-x64")) { Remove-Item (Join-Path $msixStaging "win-x64") -Recurse -Force }
if (Test-Path (Join-Path $msixStaging "publish")) { Remove-Item (Join-Path $msixStaging "publish") -Recurse -Force }

# Update manifest identity
[xml]$manifest = Get-Content $manifestPath
$identityNode = $manifest.SelectSingleNode("//*[local-name()='Identity']")
$publisherNode = $manifest.SelectSingleNode("//*[local-name()='PublisherDisplayName']")

if ($identityNode) {
    $identityNode.SetAttribute("Name", $PackageName)
    $identityNode.SetAttribute("ProcessorArchitecture", $architecture)
    $identityNode.SetAttribute("Publisher", $PublisherId)
    if ($Version) {
        $identityNode.SetAttribute("Version", $Version)
    }
}
if ($publisherNode) {
    $publisherNode.InnerText = $PublisherDisplayName
}

$manifest.Save((Join-Path $msixStaging "AppxManifest.xml"))

# Run MakeAppx
$makeAppx = (Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Recurse -Filter "makeappx.exe" | Where-Object { $_.FullName -match "x64" } | Select-Object -First 1).FullName
if ([string]::IsNullOrWhiteSpace($makeAppx) -or -not (Test-Path $makeAppx)) {
    throw "makeappx.exe was not found in the installed Windows SDK."
}
& "$makeAppx" pack /d "$msixStaging" /p "$outputMsix" /o | Out-Null
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $outputMsix)) {
    throw "makeappx failed to create $outputMsix (exit code $LASTEXITCODE)."
}

& (Join-Path $repositoryRoot "scripts\Test-MsixPackage.ps1") -PackagePath $outputMsix -StagingDirectory $msixStaging -ExpectedArchitecture $architecture
if ($LASTEXITCODE -ne 0) {
    throw "MSIX structural verification failed for $outputMsix (exit code $LASTEXITCODE)."
}

Write-Host "Store package layout generated (unsigned): $outputMsix" -ForegroundColor Green
