[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
)

$ErrorActionPreference = 'Stop'
$boundaryScript = Join-Path $RepositoryRoot 'tests\rust\portable_dependency_boundary.ps1'
$fixture = Join-Path $PSScriptRoot 'fixtures\metadata-transitive-native-dependency.json'

try {
    & $boundaryScript -RepositoryRoot $RepositoryRoot -MetadataPath $fixture
    throw 'Expected the transitive windows-core fixture to be rejected.'
}
catch {
    if ($_.Exception.Message -notmatch 'marknexia-core reaches banned native package windows-core') {
        throw "Expected a transitive native dependency rejection, got: $($_.Exception.Message)"
    }
}

Write-Host 'Portable dependency boundary transitive rejection test passed.'
