#requires -Version 7.0
[CmdletBinding()]
param([string]$SchemaPath)

$ErrorActionPreference = 'Stop'
function Assert-That([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
$generator = Join-Path $PSScriptRoot 'Generate-Sbom.ps1'
$testDirectory = Join-Path ([IO.Path]::GetTempPath()) ('marknexia-sbom-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testDirectory | Out-Null
$originalEpoch = $env:SOURCE_DATE_EPOCH
try {
    $env:SOURCE_DATE_EPOCH = '1788652800'
    $assetsPath = Join-Path $testDirectory 'project.assets.json'
    $outputPath = Join-Path $testDirectory 'sbom.json'
    $hash = [Convert]::ToBase64String([byte[]](0..63))
    $fixture = @{
        project = @{ version = '1.2.3' }
        libraries = @{
            'Direct/2.0.0' = @{ type = 'package'; sha512 = $hash }
            'Transitive/3.0.0' = @{ type = 'package'; sha512 = $hash }
            'LocalProject/1.2.3' = @{ type = 'project' }
        }
        targets = @{
            'net10.0/win-x64' = @{
                'Direct/2.0.0' = @{ type = 'package'; dependencies = @{ Transitive = '3.0.0' } }
                'Transitive/3.0.0' = @{ type = 'package' }
                'LocalProject/1.2.3' = @{ type = 'project'; dependencies = @{ Direct = '2.0.0' } }
            }
        }
    }
    $fixture | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath $assetsPath
    & $generator -AssetsPath $assetsPath -OutputPath $outputPath | Out-Null
    $firstHash = (Get-FileHash -LiteralPath $outputPath).Hash
    $document = Get-Content -LiteralPath $outputPath -Raw | ConvertFrom-Json
    Assert-That ($document.packages.Count -eq 3) 'Expected app and two NuGet packages, excluding local projects.'
    Assert-That ($document.files.Count -eq 1) 'Bundled Mermaid must be inventoried.'
    $ids = @($document.SPDXID) + @($document.packages.SPDXID) + @($document.files.SPDXID)
    Assert-That (@($ids | Select-Object -Unique).Count -eq $ids.Count) 'Duplicate SPDX IDs.'
    foreach ($edge in $document.relationships) {
        Assert-That ($ids -ccontains $edge.spdxElementId) "Undefined relationship origin: $($edge.spdxElementId)"
        Assert-That ($ids -ccontains $edge.relatedSpdxElement) "Undefined relationship target: $($edge.relatedSpdxElement)"
    }
    $direct = $document.packages | Where-Object name -eq Direct
    $transitive = $document.packages | Where-Object name -eq Transitive
    Assert-That ($direct.checksums[0].checksumValue -ceq [Convert]::ToHexString([byte[]](0..63)).ToLowerInvariant()) 'Restore package checksum changed.'
    Assert-That ($direct.externalRefs[0].referenceLocator -ceq 'pkg:nuget/Direct@2.0.0') 'Missing package URL.'
    Assert-That (@($document.relationships | Where-Object { $_.spdxElementId -eq $direct.SPDXID -and $_.relatedSpdxElement -eq $transitive.SPDXID }).Count -eq 1) 'Missing transitive dependency edge.'
    Assert-That ($direct.downloadLocation -eq 'NOASSERTION') 'Must not fabricate a public feed origin.'
    if ($SchemaPath) {
        Assert-That (Test-Json -LiteralPath $outputPath -SchemaFile $SchemaPath) 'SPDX schema validation failed.'
    }
    & $generator -AssetsPath $assetsPath -OutputPath $outputPath | Out-Null
    Assert-That ((Get-FileHash -LiteralPath $outputPath).Hash -ceq $firstHash) 'Repeated generation is not byte-identical.'
    & $generator -AssetsPath $assetsPath -OutputPath $outputPath -ProductVersion '1.2.4' | Out-Null
    $newDocument = Get-Content -LiteralPath $outputPath -Raw | ConvertFrom-Json
    Assert-That ($newDocument.documentNamespace -cne $document.documentNamespace) 'Namespace must change with document content.'

    $fixture.targets['net10.0/win-x64']['Direct/2.0.0'].dependencies['Missing'] = '9.0'
    $fixture | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath $assetsPath
    $failed = $false
    try { & $generator -AssetsPath $assetsPath -OutputPath $outputPath | Out-Null } catch { $failed = $_.Exception.Message -like '*Unresolved dependency Missing*' }
    Assert-That $failed 'Unresolved dependency must fail generation.'
    $failed = $false
    try { & $generator -AssetsPath (Join-Path $testDirectory 'missing.json') -OutputPath $outputPath | Out-Null } catch { $failed = $_.Exception.Message -like '*Restore the application first*' }
    Assert-That $failed 'Missing restore must fail generation.'
    Write-Output 'SBOM regressions passed: graph integrity, project exclusion, hashes, bundled file, package URLs, reproducibility, namespaces, and missing-input failures.'
} finally {
    $env:SOURCE_DATE_EPOCH = $originalEpoch
    # Only remove this test run's exact, newly-created temporary directory.
    $resolved = [IO.Path]::GetFullPath($testDirectory)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $resolved) -match '^marknexia-sbom-[0-9a-f]{32}$') {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
