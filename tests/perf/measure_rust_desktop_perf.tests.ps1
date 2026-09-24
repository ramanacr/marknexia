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
    $scenarioDigest = (Get-FileHash -LiteralPath $scenario -Algorithm SHA256).Hash.ToLowerInvariant()
    $provenance = @{
        commit=$artifact.commit; fixtureDigest=$artifact.fixtureDigest
        scenario=@{ id='empty-shell'; sha256=$scenarioDigest }; architecture=$artifact.architecture
        nativeArtifact=$artifact.nativeArtifact; hardware=$hardware; operatingSystem=$os
    }
    $sampleDocument = @{ cold=$cold; warm=$warm; webViewResidency='separate-process'; provenance=$provenance }
    $sampleDocument | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $samplesPath
    & $script -Scenario $scenario -SamplesPath $samplesPath -ArtifactMeasurementPath $artifactPath -OutputPath $outputPath | Out-Null
    Assert-That (Test-Json -LiteralPath $outputPath -SchemaFile $schema) 'Thirty cold and warm runs must produce schema-valid desktop evidence.'
    $result = Get-Content -LiteralPath $outputPath -Raw | ConvertFrom-Json -AsHashtable
    Assert-That ($result.runs.cold.Count -eq 30 -and $result.runs.warm.Count -eq 30) 'Desktop evidence must retain each separate sample.'
    Assert-That ($result.metrics.shellVisibleMs.median -eq 15.5) 'Median must be derived from per-run cold samples.'
    Assert-That ($result.metrics.shellVisibleMs.p95 -eq 29) 'P95 must be derived from per-run cold samples.'
    Assert-That ($result.sampleSource -eq 'imported-external' -and $result.runtimeCollectorStatus -eq 'unavailable-deferred') 'Imported metrics must retain the collector availability boundary.'
    Assert-That ($result.sampleProvenance.commit -eq $artifact.commit -and $result.sampleProvenance.scenario.sha256 -eq $scenarioDigest) 'Desktop evidence must retain sample provenance.'
    $short = Get-Content -LiteralPath $samplesPath -Raw | ConvertFrom-Json -AsHashtable
    $short.warm = @($short.warm | Select-Object -First 29)
    $short | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $samplesPath
    $rejected = $false
    try { & $script -Scenario $scenario -SamplesPath $samplesPath -ArtifactMeasurementPath $artifactPath -OutputPath $outputPath | Out-Null }
    catch { $rejected = $_.Exception.Message -like '*30 warm*' }
    Assert-That $rejected 'Desktop evidence must reject 29 warm samples.'
    $provenanceCases = @(
        @{ name='commit'; expected='*commit does not match*'; mutate={ param($value) $value.provenance.commit='fedcba9876543210' } },
        @{ name='fixture digest'; expected='*fixture digest does not match*'; mutate={ param($value) $value.provenance.fixtureDigest=('c' * 64) } },
        @{ name='scenario digest'; expected='*scenario id or SHA-256*'; mutate={ param($value) $value.provenance.scenario.sha256=('d' * 64) } },
        @{ name='architecture'; expected='*architecture does not match*'; mutate={ param($value) $value.provenance.architecture='ARM64' } },
        @{ name='native artifact'; expected='*native artifact identity does not match*'; mutate={ param($value) $value.provenance.nativeArtifact.path='other.exe' } },
        @{ name='hardware'; expected='*hardware identity does not match*'; mutate={ param($value) $value.provenance.hardware.machineName='other-host' } },
        @{ name='operating system'; expected='*operating-system identity does not match*'; mutate={ param($value) $value.provenance.operatingSystem.build='99999' } }
    )
    foreach ($case in $provenanceCases) {
        $invalidSamples = $sampleDocument | ConvertTo-Json -Depth 12 | ConvertFrom-Json -AsHashtable
        & $case.mutate $invalidSamples
        $invalidSamples | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $samplesPath
        $provenanceRejected = $false
        try { & $script -Scenario $scenario -SamplesPath $samplesPath -ArtifactMeasurementPath $artifactPath -OutputPath $outputPath | Out-Null }
        catch { $provenanceRejected = $_.Exception.Message -like $case.expected }
        Assert-That $provenanceRejected "Imported samples must be rejected when their $($case.name) provenance differs."
    }
    Write-Host 'Rust desktop performance measurement regression tests passed.'
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) { Remove-Item -LiteralPath $temporaryRoot -Recurse -Force }
}
