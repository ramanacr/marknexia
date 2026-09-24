#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Scenario,
    [Parameter(Mandatory)][string]$SamplesPath,
    [Parameter(Mandatory)][string]$ArtifactMeasurementPath,
    [Parameter(Mandatory)][string]$OutputPath
)

$ErrorActionPreference = 'Stop'
foreach ($path in @($Scenario, $SamplesPath, $ArtifactMeasurementPath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Measurement input not found: $path" }
}
$scenarioData = Get-Content -LiteralPath $Scenario -Raw | ConvertFrom-Json -AsHashtable
$samples = Get-Content -LiteralPath $SamplesPath -Raw | ConvertFrom-Json -AsHashtable
$artifact = Get-Content -LiteralPath $ArtifactMeasurementPath -Raw | ConvertFrom-Json -AsHashtable
if ($artifact.kind -ne 'artifact-measurement-v1' -or -not (Test-Json -LiteralPath $ArtifactMeasurementPath -SchemaFile (Join-Path $PSScriptRoot '..\tests\perf\rust-feasibility.schema.json'))) {
    throw 'Artifact measurement must satisfy artifact-measurement-v1 before desktop samples are imported.'
}
if ([string]::IsNullOrWhiteSpace($scenarioData.id)) { throw 'Scenario must have an id.' }
$scenarioSha256 = (Get-FileHash -LiteralPath $Scenario -Algorithm SHA256).Hash.ToLowerInvariant()
if (-not $samples.ContainsKey('provenance') -or $samples.provenance -isnot [System.Collections.IDictionary]) {
    throw 'Imported samples must include provenance for the measured build, scenario, artifact, hardware, and operating system.'
}
$provenance = $samples.provenance
foreach ($field in @('commit', 'fixtureDigest', 'scenario', 'architecture', 'nativeArtifact', 'hardware', 'operatingSystem')) {
    if (-not $provenance.ContainsKey($field)) { throw "Imported sample provenance is missing $field." }
}
$nativeArtifactRecords = @($artifact.artifacts.files | Where-Object { $_.path -ceq $artifact.nativeArtifact.path })
if ($nativeArtifactRecords.Count -ne 1) {
    throw 'Artifact measurement must contain exactly one file record for the native artifact.'
}
if (-not $provenance.ContainsKey('nativeArtifactSha256')) {
    throw 'Imported sample provenance is missing nativeArtifactSha256.'
}
if ($provenance.nativeArtifactSha256 -cne $nativeArtifactRecords[0].sha256) {
    throw 'Imported sample native artifact SHA-256 does not match the artifact measurement file record.'
}
if ($provenance.commit -cne $artifact.commit) { throw 'Imported sample commit does not match the artifact measurement.' }
if ($provenance.fixtureDigest -cne $artifact.fixtureDigest) { throw 'Imported sample fixture digest does not match the artifact measurement.' }
if ($provenance.scenario.id -cne $scenarioData.id -or $provenance.scenario.sha256 -cne $scenarioSha256) {
    throw 'Imported sample scenario id or SHA-256 does not match the supplied scenario.'
}
if ($provenance.architecture -cne $artifact.architecture) { throw 'Imported sample architecture does not match the artifact measurement.' }
foreach ($field in @('path', 'machine', 'evidenceType', 'nativeRuntimeVerified')) {
    if ($provenance.nativeArtifact[$field] -cne $artifact.nativeArtifact[$field]) {
        throw "Imported sample native artifact identity does not match ($field)."
    }
}
foreach ($field in @('machineName', 'processor', 'logicalProcessorCount', 'physicalMemoryBytes')) {
    if ($provenance.hardware[$field] -cne $artifact.hardware[$field]) {
        throw "Imported sample hardware identity does not match ($field)."
    }
}
foreach ($field in @('caption', 'build')) {
    if ($provenance.operatingSystem[$field] -cne $artifact.operatingSystem[$field]) {
        throw "Imported sample operating-system identity does not match ($field)."
    }
}
if ($samples.webViewResidency -ne 'separate-process') { throw 'WebView2 residency must identify a separate process.' }
if (@($samples.cold).Count -ne 30) { throw 'Desktop evidence requires exactly 30 cold samples.' }
if (@($samples.warm).Count -ne 30) { throw 'Desktop evidence requires exactly 30 warm samples.' }

$metricNames = @('shellVisibleMs', 'webViewReadyMs', 'firstRenderMs', 'hostPrivateBytes', 'webView2WorkingSetBytes')
foreach ($phase in @('cold', 'warm')) {
    foreach ($sample in $samples[$phase]) {
        foreach ($metric in $metricNames) {
            if (-not $sample.ContainsKey($metric) -or $null -eq $sample[$metric] -or
                $sample[$metric] -isnot [ValueType] -or [double]$sample[$metric] -lt 0) {
                throw "Every $phase sample must contain a nonnegative numeric $metric."
            }
        }
    }
}

function Get-Summary([object[]]$Values) {
    $sorted = @($Values | Sort-Object)
    $median = ([double]$sorted[14] + [double]$sorted[15]) / 2
    $p95 = [double]$sorted[28]
    return [ordered]@{ median = $median; p95 = $p95 }
}

function Get-PhaseMetrics([object[]]$PhaseSamples) {
    $result = [ordered]@{}
    foreach ($metric in $metricNames) {
        $result[$metric] = Get-Summary @($PhaseSamples | ForEach-Object { $_[$metric] })
    }
    return $result
}

$result = [ordered]@{
    schemaVersion = 'rust-feasibility-v1'
    kind = 'desktop-performance-measurement-v1'
    hardware = $artifact.hardware
    operatingSystem = $artifact.operatingSystem
    architecture = $artifact.architecture
    nativeArtifact = $artifact.nativeArtifact
    commit = $artifact.commit
    fixtureDigest = $artifact.fixtureDigest
    runCount = 60
    webViewResidency = 'separate-process'
    sampleSource = 'imported-external'
    runtimeCollectorStatus = 'unavailable-deferred'
    sampleProvenance = $provenance
    scenario = [ordered]@{
        id = $scenarioData.id
        sha256 = $scenarioSha256
    }
    runs = [ordered]@{ cold = @($samples.cold); warm = @($samples.warm) }
    metrics = Get-PhaseMetrics @($samples.cold)
    warmMetrics = Get-PhaseMetrics @($samples.warm)
    artifacts = $artifact.artifacts
}
$resolvedOutput = [IO.Path]::GetFullPath($OutputPath)
if ($resolvedOutput -in @([IO.Path]::GetFullPath($Scenario), [IO.Path]::GetFullPath($SamplesPath), [IO.Path]::GetFullPath($ArtifactMeasurementPath))) {
    throw 'OutputPath must not overwrite a measurement input.'
}
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($resolvedOutput)) | Out-Null
$result | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath $resolvedOutput -Encoding utf8NoBOM
if (-not (Test-Json -LiteralPath $resolvedOutput -SchemaFile (Join-Path $PSScriptRoot '..\tests\perf\rust-feasibility.schema.json'))) {
    throw 'Generated desktop measurement did not satisfy the feasibility schema.'
}
Write-Host "Rust desktop measurement written to $resolvedOutput (30 cold and 30 warm samples)."
