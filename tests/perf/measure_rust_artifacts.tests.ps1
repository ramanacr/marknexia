[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
)

$ErrorActionPreference = 'Stop'

function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) {
        throw $Message
    }
}

function New-PeFixture([string]$Path, [uint16]$Machine) {
    Assert-That ([IO.Path]::GetExtension($Path) -eq '.pefixture') 'Synthetic PE bytes require an inert .pefixture name.'
    $bytes = [byte[]]::new(256)
    $bytes[0] = 0x4d
    $bytes[1] = 0x5a
    [BitConverter]::GetBytes([int]128).CopyTo($bytes, 0x3c)
    $bytes[128] = 0x50
    $bytes[129] = 0x45
    [BitConverter]::GetBytes($Machine).CopyTo($bytes, 132)
    [BitConverter]::GetBytes([uint16]2).CopyTo($bytes, 148)
    [BitConverter]::GetBytes([uint16]0x20b).CopyTo($bytes, 152)
    [IO.File]::WriteAllBytes($Path, $bytes)
}

$script = Join-Path $RepositoryRoot 'scripts\Measure-RustArtifacts.ps1'
$schema = Join-Path $RepositoryRoot 'tests\perf\rust-feasibility.schema.json'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('marknexia-rust-artifacts-' + [Guid]::NewGuid().ToString('N'))

try {
    Assert-That (Test-Path -LiteralPath $script -PathType Leaf) 'Measure-RustArtifacts.ps1 must exist.'
    Assert-That (Test-Path -LiteralPath $schema -PathType Leaf) 'Rust feasibility schema must exist.'

    $scriptCommand = Get-Command -Name $script
    foreach ($name in @('Commit', 'FixtureDigest', 'RunCount')) {
        $mandatory = @($scriptCommand.Parameters[$name].Attributes | Where-Object {
            $_ -is [System.Management.Automation.ParameterAttribute] -and $_.Mandatory
        }).Count -gt 0
        Assert-That $mandatory "$name must be supplied explicitly; release evidence must not use placeholder defaults."
    }

    $artifactRoot = Join-Path $temporaryRoot 'artifacts'
    $outputPath = Join-Path $temporaryRoot 'measurement.json'
    [IO.Directory]::CreateDirectory($artifactRoot) | Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $artifactRoot 'assets')) | Out-Null
    $nativeArtifact = Join-Path $artifactRoot 'marknexia-win32.pefixture'
    $wrongArchitectureArtifact = Join-Path $artifactRoot 'arm64.pefixture'
    New-PeFixture $nativeArtifact 0x8664
    New-PeFixture $wrongArchitectureArtifact 0xaa64
    [IO.File]::WriteAllBytes((Join-Path $artifactRoot 'loader.bin'), [byte[]](1..29))
    [IO.File]::WriteAllBytes((Join-Path $artifactRoot 'assets\probe.html'), [byte[]](1..7))

    $fixtureDigest = 'a' * 64
    $hardwareJson = '{"machineName":"test-host","processor":"test-cpu","logicalProcessorCount":8,"physicalMemoryBytes":17179869184}'
    $operatingSystemJson = '{"caption":"Windows Test","build":"26100"}'
    & $script -ArtifactsRoot $artifactRoot -NativeArtifactPath $nativeArtifact -OutputPath $outputPath -Commit '0123456789abcdef' -FixtureDigest $fixtureDigest -Architecture x64 -RunCount 1 -HardwareJson $hardwareJson -OperatingSystemJson $operatingSystemJson
    $scriptSucceeded = $?
    Assert-That $scriptSucceeded 'Artifact measurement script must succeed for controlled artifacts.'
    Assert-That (Test-Path -LiteralPath $outputPath -PathType Leaf) 'Artifact measurement output was not created.'
    Assert-That (Test-Json -LiteralPath $outputPath -SchemaFile $schema) 'Artifact measurement must satisfy the Rust feasibility schema.'

    $measurement = Get-Content -LiteralPath $outputPath -Raw | ConvertFrom-Json -AsHashtable
    Assert-That ($measurement.kind -eq 'artifact-measurement-v1') 'Artifact measurement kind must identify its contract.'
    Assert-That ($measurement.artifacts.totalBytes -eq 548) 'Artifact sizes must equal the sum of controlled input bytes.'
    Assert-That ($measurement.artifacts.files.Count -eq 4) 'Every controlled artifact must be recorded.'
    Assert-That ($measurement.nativeArtifact.evidenceType -eq 'static-pe-header-only') 'Architecture proof must be identified as static PE evidence.'
    Assert-That ($measurement.nativeArtifact.machine -eq '0x8664') 'Architecture proof must reflect the measured PE header.'

    $wrongArchitectureRejected = $false
    try {
        & $script -ArtifactsRoot $artifactRoot -NativeArtifactPath $wrongArchitectureArtifact -OutputPath $outputPath -Commit '0123456789abcdef' -FixtureDigest $fixtureDigest -Architecture x64 -RunCount 1 -HardwareJson $hardwareJson -OperatingSystemJson $operatingSystemJson | Out-Null
    }
    catch { $wrongArchitectureRejected = $_.Exception.Message -like '*PE machine mismatch*' }
    Assert-That $wrongArchitectureRejected 'Artifact measurement must reject a native PE whose machine disagrees with the claimed architecture.'

    $nestedOutput = Join-Path $artifactRoot 'measurement.json'
    $nestedOutputRejected = $false
    try {
        & $script -ArtifactsRoot $artifactRoot -NativeArtifactPath $nativeArtifact -OutputPath $nestedOutput -Commit '0123456789abcdef' -FixtureDigest $fixtureDigest -Architecture x64 -RunCount 1 -HardwareJson $hardwareJson -OperatingSystemJson $operatingSystemJson | Out-Null
    }
    catch { $nestedOutputRejected = $_.Exception.Message -like '*OutputPath*ArtifactsRoot*' }
    Assert-That $nestedOutputRejected 'Artifact measurement must reject OutputPath within ArtifactsRoot.'
    Assert-That (-not (Test-Path -LiteralPath $nestedOutput)) 'Rejected output must not be written into the measured artifacts.'
    Assert-That ($measurement.webViewResidency -eq 'not-measured') 'Artifact-only evidence must not invent WebView2 residency.'
    Assert-That ($measurement.metrics.hostPrivateBytes -eq $null) 'Artifact-only evidence must keep host memory separate and unmeasured.'
    Assert-That ($measurement.metrics.webView2WorkingSetBytes -eq $null) 'Artifact-only evidence must keep WebView2 memory separate and unmeasured.'

    $incomplete = $measurement.Clone()
    $incomplete.Remove('fixtureDigest')
    $incompletePath = Join-Path $temporaryRoot 'incomplete.json'
    $incomplete | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $incompletePath -NoNewline
    Assert-That (-not (Test-Json -LiteralPath $incompletePath -SchemaFile $schema -ErrorAction SilentlyContinue)) 'Measurements missing fixture digest must be rejected.'

    $unmeasuredDesktop = $measurement.Clone()
    $unmeasuredDesktop.kind = 'desktop-performance-measurement-v1'
    $unmeasuredDesktop.webViewResidency = 'separate-process'
    $unmeasuredPath = Join-Path $temporaryRoot 'unmeasured-desktop.json'
    $unmeasuredDesktop | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $unmeasuredPath -NoNewline
    Assert-That (-not (Test-Json -LiteralPath $unmeasuredPath -SchemaFile $schema -ErrorAction SilentlyContinue)) 'Desktop evidence with null timing and memory values must be rejected.'

    $summaryOnly = $measurement | ConvertTo-Json -Depth 10 | ConvertFrom-Json -AsHashtable
    $summaryOnly.kind = 'desktop-performance-measurement-v1'
    $summaryOnly.webViewResidency = 'separate-process'
    $summaryOnly.metrics.shellVisibleMs = @{ median = 100; p95 = 150 }
    $summaryOnly.metrics.webViewReadyMs = @{ median = 200; p95 = 275 }
    $summaryOnly.metrics.firstRenderMs = @{ median = 250; p95 = 330 }
    $summaryOnly.metrics.hostPrivateBytes = @{ median = 15000000; p95 = 16000000 }
    $summaryOnly.metrics.webView2WorkingSetBytes = @{ median = 40000000; p95 = 41000000 }
    $summaryOnlyPath = Join-Path $temporaryRoot 'summary-only.json'
    $summaryOnly | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $summaryOnlyPath -NoNewline
    Assert-That (-not (Test-Json -LiteralPath $summaryOnlyPath -SchemaFile $schema -ErrorAction SilentlyContinue)) 'Desktop summaries without per-run cold and warm samples must be rejected.'

    Write-Host 'Rust artifact measurement regression tests passed.'
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
