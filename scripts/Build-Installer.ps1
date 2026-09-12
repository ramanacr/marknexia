#requires -Version 7.0

[CmdletBinding()]
param(
    [ValidateSet("Release", "Debug")]
    [string]$Configuration = "Release",
    [ValidateSet("x64", "ARM64")]
    [string]$Platform = "x64",
    [ValidatePattern("^\d+\.\d+\.\d+$")]
    [string]$Version,
    [string]$OutputDirectory = "artifacts/installers",
    [switch]$NoRestore,
    [switch]$SkipTest
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$appOutput = Join-Path $repositoryRoot "src\Marknexia.App\bin\$Platform\$Configuration\net10.0-windows10.0.19041.0"
$setupProject = Join-Path $repositoryRoot "src\Marknexia.Setup\Marknexia.Setup.csproj"
$outputRoot = if ([IO.Path]::IsPathRooted($OutputDirectory)) { $OutputDirectory } else { Join-Path $repositoryRoot $OutputDirectory }
$assetToken = if ($Platform -eq "ARM64") { "arm64" } else { "x64" }
$runtime = if ($Platform -eq "ARM64") { "win-arm64" } else { "win-x64" }

if ([string]::IsNullOrWhiteSpace($Version)) {
    $props = Get-Content (Join-Path $repositoryRoot "Directory.Build.props") -Raw
    $major = [regex]::Match($props, '<VersionMajor[^>]*>(\d+)</VersionMajor>').Groups[1].Value
    $minor = [regex]::Match($props, '<VersionMinor[^>]*>(\d+)</VersionMinor>').Groups[1].Value
    $patch = [regex]::Match($props, '<VersionPatch[^>]*>(\d+)</VersionPatch>').Groups[1].Value
    $Version = "$major.$minor.$patch"
}

if (-not (Test-Path (Join-Path $appOutput "Marknexia.App.exe") -PathType Leaf)) {
    throw "Release app output was not found: $appOutput"
}
foreach ($requiredFile in @("Marknexia.App.dll", "Marknexia.App.pri", "marknexia-sbom.spdx.json")) {
    if (-not (Test-Path (Join-Path $appOutput $requiredFile) -PathType Leaf)) {
        throw "Release app output is missing required installer payload file: $requiredFile"
    }
}

$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) "marknexia-setup-$([Guid]::NewGuid().ToString('N'))"
$payloadPath = Join-Path $temporaryRoot "payload.zip"
$payloadSource = Join-Path $temporaryRoot "payload-source"
$publishDirectory = Join-Path $temporaryRoot "publish"
$installerPath = Join-Path $outputRoot "Marknexia-v$Version-win-$assetToken-setup.exe"
try {
    New-Item -ItemType Directory -Path $temporaryRoot -Force | Out-Null
    New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null
    New-Item -ItemType Directory -Path $payloadSource -Force | Out-Null
    Get-ChildItem -LiteralPath $appOutput -File -Recurse | Where-Object Extension -ne ".pdb" | ForEach-Object {
        $relativePath = $_.FullName.Substring($appOutput.Length).TrimStart([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
        $destination = Join-Path $payloadSource $relativePath
        New-Item -ItemType Directory -Path (Split-Path $destination -Parent) -Force | Out-Null
        Copy-Item -LiteralPath $_.FullName -Destination $destination -Force
    }
    Compress-Archive -Path (Join-Path $payloadSource "*") -DestinationPath $payloadPath -CompressionLevel Optimal

    $versionParts = $Version.Split('.')
    $publishArgs = @(
        "publish", $setupProject,
        "--configuration", $Configuration,
        "--runtime", $runtime,
        "--self-contained", "true",
        "--output", $publishDirectory,
        "-p:InstallerPayload=$payloadPath",
        "-p:Version=$Version",
        "-p:PackageVersion=$Version",
        "-p:VersionMajor=$($versionParts[0])",
        "-p:VersionMinor=$($versionParts[1])",
        "-p:VersionPatch=$($versionParts[2])",
        "-p:VersionOverride=true"
    )
    if ($NoRestore) { $publishArgs += "--no-restore" }
    & dotnet @publishArgs
    if ($LASTEXITCODE -ne 0) { throw "Custom installer publish failed ($LASTEXITCODE)." }

    $publishedSetup = Join-Path $publishDirectory "MarknexiaSetup.exe"
    if (-not (Test-Path $publishedSetup -PathType Leaf)) { throw "Custom installer output was not found: $publishedSetup" }
    Copy-Item $publishedSetup $installerPath -Force
    (Get-FileHash $installerPath -Algorithm SHA256).Hash | Set-Content "$installerPath.sha256" -NoNewline

    $hostArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    if (-not $SkipTest -and ($assetToken -eq $hostArchitecture)) {
        & (Join-Path $PSScriptRoot "Test-Installer.ps1") -InstallerPath $installerPath -ExpectedArchitecture $assetToken
        if ($LASTEXITCODE -ne 0) { throw "Custom installer verification failed ($LASTEXITCODE)." }
    } elseif (-not $SkipTest) {
        & (Join-Path $PSScriptRoot "Test-Installer.ps1") -InstallerPath $installerPath -ExpectedArchitecture $assetToken -StaticOnly
        if ($LASTEXITCODE -ne 0) { throw "Custom installer static verification failed ($LASTEXITCODE)." }
    }
    Write-Host "Custom installer generated: $installerPath" -ForegroundColor Green
}
finally {
    if (Test-Path $temporaryRoot) { Remove-Item -LiteralPath $temporaryRoot -Recurse -Force -ErrorAction SilentlyContinue }
}
