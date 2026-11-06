#requires -Version 7.0
<#
.SYNOPSIS
CI only: builds the unsigned, unpublished portable preview ZIP for one architecture,
with SHA256SUMS, the SPDX SBOM and THIRD-PARTY-NOTICES.txt, and verifies them.

.DESCRIPTION
Runs only on GitHub Actions runners. It refuses to run anywhere else, because it puts a
real release executable into an archive (local antivirus policy, see the production plan).

1. Confirms that the two clean release builds are byte-identical (Compare-RustBuilds.ps1).
2. Stages marknexia-win32.exe from the first build and the pinned WebView2Loader.dll
   (Stage-WebView2Loader.ps1), plus README.txt.
3. Writes THIRD-PARTY-NOTICES.txt into the staging folder and the SBOM next to it
   (Generate-RustSbom.ps1).
4. Writes a deterministic ZIP (sorted entries, SOURCE_DATE_EPOCH timestamps) and SHA256SUMS.
5. Extracts the ZIP to a fresh folder and checks every entry hash against staging, the shell
   against both builds, the loader against its lock, the SBOM (Test-RustSbom.ps1) and SHA256SUMS.
Nothing is signed, released or published.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')][string]$Target,
    [Parameter(Mandatory)][string]$FirstRoot,
    [Parameter(Mandatory)][string]$SecondRoot,
    [Parameter(Mandatory)][string]$LoaderPackagePath,
    [Parameter(Mandatory)][string]$MetadataPath,
    [Parameter(Mandatory)][string]$OutputRoot,
    [string]$SourceCommit = 'NOASSERTION'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'New-RustPortablePackage.ps1 runs only on GitHub Actions runners. Do not archive executables on a local machine.'
}
if (-not $env:SOURCE_DATE_EPOCH) { throw 'Set SOURCE_DATE_EPOCH to the source commit time for a reproducible archive.' }

$architecture = if ($Target -eq 'x86_64-pc-windows-msvc') { 'x64' } else { 'ARM64' }
$shellName = 'marknexia-win32.exe'
$loaderName = 'WebView2Loader.dll'
$repoRoot = Split-Path -Parent $PSScriptRoot
$loaderLockPath = Join-Path $repoRoot 'packaging\webview\loader.lock.json'
$loaderLock = Get-Content -LiteralPath $loaderLockPath -Raw | ConvertFrom-Json -AsHashtable
$metadata = Get-Content -LiteralPath $MetadataPath -Raw | ConvertFrom-Json -AsHashtable
$version = @($metadata.packages | Where-Object { $_.name -ceq 'marknexia-win32' -and $metadata.workspace_members -contains $_.id })[0].version
$baseName = "marknexia-win32-$version-$($architecture.ToLowerInvariant())-unsigned-preview"
# Ordinal sort, independent of the runner's culture, so output is byte-stable.
function Select-Ordinal([object[]]$Items, [scriptblock]$Key = { $_ }) {
    if (-not $Items -or $Items.Count -eq 0) { return , @() }
    $keys = [string[]]@($Items | ForEach-Object -Process $Key | ForEach-Object { [string]$_ })
    if ($keys.Count -ne $Items.Count) { throw 'Sort key must yield exactly one value per item.' }
    $sorted = [object[]]$Items.Clone()
    # Cast explicitly: the generic overload binds a converted copy of $sorted and leaves it unsorted.
    [Array]::Sort([Array]$keys, [Array]$sorted, [Collections.IComparer][StringComparer]::Ordinal)
    , $sorted
}
function Get-Digest([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

$outputFull = [IO.Path]::GetFullPath($OutputRoot)
$staging = Join-Path $outputFull "staging\$baseName"
$work = Join-Path $outputFull 'work'
$out = Join-Path $outputFull 'out'
foreach ($directory in $staging, $work, $out) {
    if (Test-Path -LiteralPath $directory) { throw "Output folder must not exist yet: $directory" }
    [IO.Directory]::CreateDirectory($directory) | Out-Null
}

# 1. Both clean builds must be byte-identical before anything is packaged.
& (Join-Path $PSScriptRoot 'Compare-RustBuilds.ps1') -FirstRoot $FirstRoot -SecondRoot $SecondRoot -OutputPath (Join-Path $out 'unsigned-reproducibility.json')
$firstShell = Get-Digest (Join-Path $FirstRoot $shellName)
$secondShell = Get-Digest (Join-Path $SecondRoot $shellName)
if ($firstShell -ne $secondShell) { throw 'The two release builds differ.' }

# 2. Stage the package files.
Copy-Item -LiteralPath (Join-Path $FirstRoot $shellName) -Destination (Join-Path $staging $shellName)
& (Join-Path $PSScriptRoot 'Stage-WebView2Loader.ps1') -Architecture $architecture -PackagePath $LoaderPackagePath -OutputPath (Join-Path $staging $loaderName) -LockPath $loaderLockPath
$readme = @"
Marknexia Rust/Win32 shell $version for Windows $architecture - UNSIGNED PREVIEW

This is an unsigned, unpublished preview build produced by continuous integration
for evidence only. It is not a release. Windows SmartScreen and antivirus products can
warn about unsigned executables. The supported product is the .NET edition of Marknexia.

Contents:
  $shellName        the Rust/Win32 shell (portable; nothing is installed)
  $loaderName         Microsoft WebView2 loader $($loaderLock.version) for $architecture
  THIRD-PARTY-NOTICES.txt    third-party components and their licenses
  README.txt                 this file

Requirement: the Microsoft Edge WebView2 Evergreen Runtime. If it is missing, the shell
offers to install it only when you ask.

Source commit: $SourceCommit
The SPDX SBOM and SHA256SUMS are published next to this ZIP in the same CI artifact.
"@
[IO.File]::WriteAllText((Join-Path $staging 'README.txt'), ($readme -replace "`r`n", "`n"), [Text.UTF8Encoding]::new($false))

# The SDK license text is read from the pinned package; nothing in it is executed.
$loaderLicense = $null
$package = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($LoaderPackagePath))
try {
    $entry = @($package.Entries | Where-Object { $_.FullName -match '^(?i)licen[cs]e(\.txt|\.md)?$' } | Sort-Object FullName) | Select-Object -First 1
    if ($entry) {
        $reader = [IO.StreamReader]::new($entry.Open())
        try { $loaderLicense = Join-Path $work 'webview2-sdk-license.txt'; [IO.File]::WriteAllText($loaderLicense, $reader.ReadToEnd()) } finally { $reader.Dispose() }
    }
} finally { $package.Dispose() }

# 3. Notices into the package, SBOM next to it.
$sbomPath = Join-Path $out "$baseName.spdx.json"
$sbomArguments = @{
    MetadataPath = $MetadataPath; PackageRoot = $staging; Architecture = $architecture; OutputPath = $sbomPath
    SourceCommit = $SourceCommit; LoaderLockPath = $loaderLockPath
}
if ($loaderLicense) { $sbomArguments.LoaderLicensePath = $loaderLicense }
& (Join-Path $PSScriptRoot 'Generate-RustSbom.ps1') @sbomArguments

# 4. Deterministic ZIP and SHA256SUMS.
$zipPath = Join-Path $out "$baseName.zip"
$timestamp = [DateTimeOffset]::FromUnixTimeSeconds([long]$env:SOURCE_DATE_EPOCH)
$stagedFiles = Select-Ordinal @(Get-ChildItem -LiteralPath $staging -File) { $_.Name }
$archive = [IO.Compression.ZipFile]::Open($zipPath, [IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($file in $stagedFiles) {
        $zipEntry = $archive.CreateEntry($file.Name, [IO.Compression.CompressionLevel]::Optimal)
        $zipEntry.LastWriteTime = $timestamp
        $target = $zipEntry.Open()
        try { $source = [IO.File]::OpenRead($file.FullName); try { $source.CopyTo($target) } finally { $source.Dispose() } } finally { $target.Dispose() }
    }
} finally { $archive.Dispose() }
$sumsPath = Join-Path $out 'SHA256SUMS'
$sums = @(Get-ChildItem -LiteralPath $out -File | Where-Object { $_.Name -in @("$baseName.zip", "$baseName.spdx.json") } |
    Sort-Object Name | ForEach-Object { "$(Get-Digest $_.FullName)  $($_.Name)" })
[IO.File]::WriteAllText($sumsPath, ($sums -join "`n") + "`n", [Text.UTF8Encoding]::new($false))

# 5. Verify what was written, from a fresh extraction.
$verify = Join-Path $work 'extracted'
[IO.Compression.ZipFile]::ExtractToDirectory($zipPath, $verify)
$expectedNames = @($stagedFiles | ForEach-Object Name)
$actualNames = Select-Ordinal @(Get-ChildItem -LiteralPath $verify -File -Recurse | ForEach-Object { [IO.Path]::GetRelativePath($verify, $_.FullName) })
if (($actualNames -join '|') -cne ($expectedNames -join '|')) { throw "ZIP entries differ from staging: $($actualNames -join ', ')" }
if (($expectedNames -join '|') -cne ((Select-Ordinal @($shellName, 'README.txt', 'THIRD-PARTY-NOTICES.txt', $loaderName)) -join '|')) {
    throw "Unexpected package layout: $($expectedNames -join ', ')"
}
$entries = foreach ($file in $stagedFiles) {
    $extracted = Get-Digest (Join-Path $verify $file.Name)
    if ($extracted -ne (Get-Digest $file.FullName)) { throw "ZIP entry $($file.Name) differs from staging." }
    [ordered]@{ path = $file.Name; sha256 = $extracted; bytes = $file.Length }
}
$zipShell = Get-Digest (Join-Path $verify $shellName)
if ($zipShell -ne $firstShell -or $zipShell -ne $secondShell) { throw 'ZIP shell does not match both reproducible builds.' }
if ((Get-Digest (Join-Path $verify $loaderName)) -ne $loaderLock.loaders[$architecture].sha256) { throw 'ZIP loader is not the pinned loader.' }
& (Join-Path $PSScriptRoot 'Test-RustSbom.ps1') -SbomPath $sbomPath -PackageRoot $verify -MetadataPath $MetadataPath -Architecture $architecture -LoaderLockPath $loaderLockPath
foreach ($line in [IO.File]::ReadAllLines($sumsPath)) {
    $hash, $name = $line -split '  ', 2
    if ((Get-Digest (Join-Path $out $name)) -ne $hash) { throw "SHA256SUMS mismatch for $name" }
}

$evidence = [ordered]@{
    schemaVersion = 'rust-unsigned-preview-package-v1'
    target = $Target
    architecture = $architecture
    version = $version
    sourceCommit = $SourceCommit
    signed = $false
    published = $false
    zip = [ordered]@{ name = "$baseName.zip"; sha256 = (Get-Digest $zipPath); bytes = (Get-Item -LiteralPath $zipPath).Length }
    sbom = [ordered]@{ name = "$baseName.spdx.json"; format = 'SPDX-2.3'; sha256 = (Get-Digest $sbomPath) }
    reproducibleShell = [ordered]@{ firstBuildSha256 = $firstShell; secondBuildSha256 = $secondShell; zipSha256 = $zipShell }
    entries = @($entries)
    unpackedBytes = [int64](($entries | Measure-Object -Property bytes -Sum).Sum)
}
$evidence | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $out 'package-verification.json') -Encoding utf8NoBOM
Write-Output "Unsigned preview package verified: $zipPath"
