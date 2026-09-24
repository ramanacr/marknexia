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
    [IO.File]::WriteAllBytes((Join-Path $artifactRoot 'marknexia-win32.exe'), [byte[]](1..17))
    [IO.File]::WriteAllBytes((Join-Path $artifactRoot 'WebView2Loader.dll'), [byte[]](1..29))
    [IO.File]::WriteAllBytes((Join-Path $artifactRoot 'assets\probe.html'), [byte[]](1..7))

    $fixtureDigest = 'a' * 64
    $hardwareJson = '{"machineName":"test-host","processor":"test-cpu","logicalProcessorCount":8,"physicalMemoryBytes":17179869184}'
    $operatingSystemJson = '{"caption":"Windows Test","build":"26100"}'
    & $script -ArtifactsRoot $artifactRoot -OutputPath $outputPath -Commit '0123456789abcdef' -FixtureDigest $fixtureDigest -Architecture x64 -RunCount 30 -HardwareJson $hardwareJson -OperatingSystemJson $operatingSystemJson
    $scriptSucceeded = $?
    Assert-That $scriptSucceeded 'Artifact measurement script must succeed for controlled artifacts.'
    Assert-That (Test-Path -LiteralPath $outputPath -PathType Leaf) 'Artifact measurement output was not created.'
    Assert-That (Test-Json -LiteralPath $outputPath -SchemaFile $schema) 'Artifact measurement must satisfy the Rust feasibility schema.'

    $measurement = Get-Content -LiteralPath $outputPath -Raw | ConvertFrom-Json -AsHashtable
    Assert-That ($measurement.kind -eq 'artifact-measurement-v1') 'Artifact measurement kind must identify its contract.'
    Assert-That ($measurement.artifacts.totalBytes -eq 53) 'Artifact sizes must equal the sum of controlled input bytes.'
    Assert-That ($measurement.artifacts.files.Count -eq 3) 'Every controlled artifact must be recorded.'
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

    $measuredDesktop = $measurement | ConvertTo-Json -Depth 10 | ConvertFrom-Json -AsHashtable
    $measuredDesktop.kind = 'desktop-performance-measurement-v1'
    $measuredDesktop.webViewResidency = 'separate-process'
    $measuredDesktop.metrics.shellVisibleMs = @{ median = 100; p95 = 150 }
    $measuredDesktop.metrics.webViewReadyMs = @{ median = 200; p95 = 275 }
    $measuredDesktop.metrics.firstRenderMs = @{ median = 250; p95 = 330 }
    $measuredDesktop.metrics.hostPrivateBytes = 15000000
    $measuredDesktop.metrics.webView2WorkingSetBytes = 40000000
    $measuredPath = Join-Path $temporaryRoot 'measured-desktop.json'
    $measuredDesktop | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $measuredPath -NoNewline
    Assert-That (Test-Json -LiteralPath $measuredPath -SchemaFile $schema) 'Complete desktop evidence must satisfy the schema.'

    Write-Host 'Rust artifact measurement regression tests passed.'
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
