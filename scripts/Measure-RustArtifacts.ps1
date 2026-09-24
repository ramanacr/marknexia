#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ArtifactsRoot,
    [Parameter(Mandatory)][string]$NativeArtifactPath,
    [Parameter(Mandatory)][string]$OutputPath,
    [ValidateSet('x64', 'ARM64')][string]$Architecture = 'x64',
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{7,64}$')][string]$Commit,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$FixtureDigest,
    [Parameter(Mandatory)][ValidateRange(1, 100000)][int]$RunCount,
    [string]$HardwareJson,
    [string]$OperatingSystemJson
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $ArtifactsRoot -PathType Container)) {
    throw "Artifact root was not found: $ArtifactsRoot"
}

$resolvedRoot = [IO.Path]::GetFullPath($ArtifactsRoot).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
$rootPrefix = $resolvedRoot + [IO.Path]::DirectorySeparatorChar
$resolvedOutput = [IO.Path]::GetFullPath($OutputPath)
if ($resolvedOutput.Equals($resolvedRoot, [StringComparison]::OrdinalIgnoreCase) -or
    $resolvedOutput.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'OutputPath must be outside ArtifactsRoot so measurement output cannot change the measured input.'
}
$resolvedNativeArtifact = [IO.Path]::GetFullPath($NativeArtifactPath)
if (-not $resolvedNativeArtifact.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'NativeArtifactPath must be a file within ArtifactsRoot.'
}
$architectureEvidence = & (Join-Path $PSScriptRoot 'Test-RustArchitecture.ps1') -Path $resolvedNativeArtifact -Expected $Architecture
$files = @(Get-ChildItem -LiteralPath $resolvedRoot -File -Recurse | Sort-Object FullName)
if ($files.Count -eq 0) {
    throw "Artifact root contains no files: $resolvedRoot"
}

$binaryExtensions = @('.exe', '.dll')
$records = foreach ($file in $files) {
    $relativePath = [IO.Path]::GetRelativePath($resolvedRoot, $file.FullName).Replace('\', '/')
    [ordered]@{
        path = $relativePath
        bytes = [int64]$file.Length
        sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

$totalBytes = [int64](($records | ForEach-Object { $_['bytes'] } | Measure-Object -Sum).Sum)
$binaryRecords = @($records | Where-Object { [IO.Path]::GetExtension($_['path']).ToLowerInvariant() -in $binaryExtensions })
$binaryBytes = [int64](($binaryRecords | ForEach-Object { $_['bytes'] } | Measure-Object -Sum).Sum)
$assetsBytes = $totalBytes - $binaryBytes

if ($HardwareJson) {
    $hardware = $HardwareJson | ConvertFrom-Json -AsHashtable
}
else {
    $computer = Get-CimInstance -ClassName Win32_ComputerSystem
    $processor = Get-CimInstance -ClassName Win32_Processor | Select-Object -First 1
    $hardware = [ordered]@{
        machineName = [Environment]::MachineName
        processor = $processor.Name.Trim()
        logicalProcessorCount = [int]$computer.NumberOfLogicalProcessors
        physicalMemoryBytes = [int64]$computer.TotalPhysicalMemory
    }
}

if ($OperatingSystemJson) {
    $operatingSystem = $OperatingSystemJson | ConvertFrom-Json -AsHashtable
}
else {
    $operatingSystemRecord = Get-CimInstance -ClassName Win32_OperatingSystem
    $operatingSystem = [ordered]@{
        caption = $operatingSystemRecord.Caption.Trim()
        build = $operatingSystemRecord.BuildNumber
    }
}

$measurement = [ordered]@{
    schemaVersion = 'rust-feasibility-v1'
    kind = 'artifact-measurement-v1'
    hardware = $hardware
    operatingSystem = $operatingSystem
    architecture = $Architecture
    nativeArtifact = [ordered]@{
        path = [IO.Path]::GetRelativePath($resolvedRoot, $resolvedNativeArtifact).Replace('\', '/')
        machine = $architectureEvidence.machine
        evidenceType = $architectureEvidence.evidenceType
        nativeRuntimeVerified = $false
    }
    commit = $Commit
    fixtureDigest = $FixtureDigest
    runCount = $RunCount
    webViewResidency = 'not-measured'
    metrics = [ordered]@{
        shellVisibleMs = [ordered]@{ median = $null; p95 = $null }
        webViewReadyMs = [ordered]@{ median = $null; p95 = $null }
        firstRenderMs = [ordered]@{ median = $null; p95 = $null }
        hostPrivateBytes = $null
        webView2WorkingSetBytes = $null
    }
    artifacts = [ordered]@{
        totalBytes = $totalBytes
        binaryBytes = $binaryBytes
        assetsBytes = $assetsBytes
        unpackedBytes = $totalBytes
        files = @($records)
    }
}

$outputDirectory = Split-Path -Parent $resolvedOutput
if ($outputDirectory) {
    [IO.Directory]::CreateDirectory($outputDirectory) | Out-Null
}
$measurement | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $resolvedOutput -Encoding utf8NoBOM
Write-Host "Rust artifact measurement written to $resolvedOutput ($totalBytes bytes across $($records.Count) files)."
