[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

# Exercises the collector's fixture handling up to, but not including, the
# shell launch: every case below fails the artifact fixture-digest binding, so
# no process starts. The "native artifact" is a plain text file (never a PE).
$ErrorActionPreference = 'Stop'
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }

$script = Join-Path $RepositoryRoot 'scripts\Collect-RustDesktopPerf.ps1'
$scenarios = Join-Path $RepositoryRoot 'tests\perf\scenarios'
$root = Join-Path ([IO.Path]::GetTempPath()) ('marknexia-collect-perf-' + [Guid]::NewGuid().ToString('N'))
try {
    [IO.Directory]::CreateDirectory($root) | Out-Null
    $artifactFile = Join-Path $root 'stand-in.txt'
    Set-Content -LiteralPath $artifactFile -Value 'not an executable' -NoNewline
    $artifactHash = (Get-FileHash -LiteralPath $artifactFile -Algorithm SHA256).Hash.ToLowerInvariant()
    $artifactPath = Join-Path $root 'artifact.json'
    @{
        commit = '0123456789abcdef'; fixtureDigest = ('0' * 64); architecture = 'x64'
        nativeArtifact = @{ path = 'stand-in.txt' }
        artifacts = @{ files = @(@{ path = 'stand-in.txt'; sha256 = $artifactHash }) }
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $artifactPath

    function Invoke-Collector([string]$ScenarioId) {
        try {
            & $script -ArtifactMeasurementPath $artifactPath -ArtifactsRoot $root `
                -Scenario (Join-Path $scenarios "$ScenarioId.json") -OutputPath (Join-Path $root 'out.json') -Runs 1 | Out-Null
        }
        catch { return $_.Exception.Message }
        throw "Collector unexpectedly succeeded for $ScenarioId."
    }
    function Get-ReportedDigest([string]$Message) {
        $match = [regex]::Match($Message, "fixture digest '([0-9a-f]{64})'")
        Assert-That $match.Success "Digest mismatch message expected, got: $Message"
        return $match.Groups[1].Value
    }

    $features = Join-Path $RepositoryRoot 'test-fixtures\markdown\gfm\features.md'
    $small = Get-ReportedDigest (Invoke-Collector 'small-document')
    Assert-That ($small -ceq (Get-FileHash -LiteralPath $features -Algorithm SHA256).Hash.ToLowerInvariant()) 'small-document must digest the pinned fixture.'

    $before = @(Get-ChildItem ([IO.Path]::GetTempPath()) -Filter 'marknexia-perf-large-document-*.md').Count
    $large = Get-ReportedDigest (Invoke-Collector 'large-document')
    $scenario = Get-Content (Join-Path $scenarios 'large-document.json') -Raw | ConvertFrom-Json
    $bytes = [IO.File]::ReadAllBytes($features)
    $expected = [IO.MemoryStream]::new()
    foreach ($index in 1..$scenario.fixture.repeat) { $expected.Write($bytes, 0, $bytes.Length); $expected.WriteByte(0x0A) }
    $hash = [Security.Cryptography.SHA256]::HashData($expected.ToArray())
    Assert-That ($large -ceq [Convert]::ToHexString($hash).ToLowerInvariant()) 'large-document must digest source+LF repeated per the scenario.'
    $after = @(Get-ChildItem ([IO.Path]::GetTempPath()) -Filter 'marknexia-perf-large-document-*.md').Count
    Assert-That ($after -le $before) 'The generated large document must be removed when collection stops.'

    $scan = Invoke-Collector 'repository-scan'
    Assert-That ($scan -like "*repository-scan*not implement*") "repository-scan must fail clearly, got: $scan"
    Write-Host 'Collect-RustDesktopPerf fixture tests passed.'
}
finally {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
