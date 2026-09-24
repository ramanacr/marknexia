[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

$ErrorActionPreference = 'Stop'
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }

$script = Join-Path $RepositoryRoot 'scripts\Measure-RustDesktopPerf.ps1'
$schema = Join-Path $RepositoryRoot 'tests\perf\rust-feasibility.schema.json'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('marknexia-desktop-perf-' + [Guid]::NewGuid().ToString('N'))
try {
    [IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    $scenario = Join-Path $RepositoryRoot 'tests\perf\scenarios\empty-shell.json'
    $artifactPath = Join-Path $temporaryRoot 'artifacts.json'
    $samplesPath = Join-Path $temporaryRoot 'samples.json'
    $outputPath = Join-Path $temporaryRoot 'desktop.json'
    $hardware = @{ machineName='test-host'; processor='test-cpu'; logicalProcessorCount=8; physicalMemoryBytes=17179869184 }
    $os = @{ caption='Windows Test'; build='26100' }
    $artifact = @{
        schemaVersion='rust-feasibility-v1'; kind='artifact-measurement-v1'; hardware=$hardware; operatingSystem=$os
        architecture='x64'; nativeArtifact=@{ path='marknexia-win32.exe'; machine='0x8664'; evidenceType='static-pe-header-only'; nativeRuntimeVerified=$false }
        commit='0123456789abcdef'; fixtureDigest=('a' * 64); runCount=1; webViewResidency='not-measured'
        metrics=@{ shellVisibleMs=@{median=$null;p95=$null}; webViewReadyMs=@{median=$null;p95=$null}; firstRenderMs=@{median=$null;p95=$null}; hostPrivateBytes=$null; webView2WorkingSetBytes=$null }
        artifacts=@{ totalBytes=256;binaryBytes=256;assetsBytes=0;unpackedBytes=256;files=@(@{path='marknexia-win32.exe';bytes=256;sha256=('b' * 64)}) }
    }
    $artifact | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $artifactPath
    $cold = @(1..30 | ForEach-Object { @{ shellVisibleMs=$_;webViewReadyMs=($_+10);firstRenderMs=($_+20);hostPrivateBytes=15000000;webView2WorkingSetBytes=40000000 } })
    $warm = @(1..30 | ForEach-Object { @{ shellVisibleMs=($_+1);webViewReadyMs=($_+11);firstRenderMs=($_+21);hostPrivateBytes=16000000;webView2WorkingSetBytes=41000000 } })
    @{ cold=$cold; warm=$warm; webViewResidency='separate-process' } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $samplesPath
    & $script -Scenario $scenario -SamplesPath $samplesPath -ArtifactMeasurementPath $artifactPath -OutputPath $outputPath | Out-Null
    Assert-That (Test-Json -LiteralPath $outputPath -SchemaFile $schema) 'Thirty cold and warm runs must produce schema-valid desktop evidence.'
    $result = Get-Content -LiteralPath $outputPath -Raw | ConvertFrom-Json -AsHashtable
    Assert-That ($result.runs.cold.Count -eq 30 -and $result.runs.warm.Count -eq 30) 'Desktop evidence must retain each separate sample.'
    Assert-That ($result.metrics.shellVisibleMs.median -eq 15.5) 'Median must be derived from per-run cold samples.'
    Assert-That ($result.metrics.shellVisibleMs.p95 -eq 29) 'P95 must be derived from per-run cold samples.'
    $short = Get-Content -LiteralPath $samplesPath -Raw | ConvertFrom-Json -AsHashtable
    $short.warm = @($short.warm | Select-Object -First 29)
    $short | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $samplesPath
    $rejected = $false
    try { & $script -Scenario $scenario -SamplesPath $samplesPath -ArtifactMeasurementPath $artifactPath -OutputPath $outputPath | Out-Null }
    catch { $rejected = $_.Exception.Message -like '*30 warm*' }
    Assert-That $rejected 'Desktop evidence must reject 29 warm samples.'
    Write-Host 'Rust desktop performance measurement regression tests passed.'
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) { Remove-Item -LiteralPath $temporaryRoot -Recurse -Force }
}
