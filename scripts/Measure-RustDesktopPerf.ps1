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
    scenario = [ordered]@{
        id = $scenarioData.id
        sha256 = (Get-FileHash -LiteralPath $Scenario -Algorithm SHA256).Hash.ToLowerInvariant()
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
