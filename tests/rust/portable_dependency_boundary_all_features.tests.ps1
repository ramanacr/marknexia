[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
)

$ErrorActionPreference = 'Stop'
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) "Marknexia Boundary $([Guid]::NewGuid())"

try {
    [System.IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    $cargo = Join-Path $temporaryRoot 'cargo fixture.cmd'
    $defaultFixture = Join-Path $PSScriptRoot 'fixtures\metadata-feature-gated-native-dependency-default.json'
    $allFeaturesFixture = Join-Path $PSScriptRoot 'fixtures\metadata-feature-gated-native-dependency-all-features.json'
    $nativeOnlyFixture = Join-Path $PSScriptRoot 'fixtures\metadata-native-crate-native-dependency.json'
    $boundaryScript = Join-Path $RepositoryRoot 'tests\rust\portable_dependency_boundary.ps1'

    [System.IO.File]::WriteAllLines($cargo, @(
        '@echo off',
        'echo %* | findstr /c:"--all-features" >nul',
        "if errorlevel 1 (type `"$defaultFixture`") else (type `"$allFeaturesFixture`")",
        'exit /b 0'
    ), [System.Text.Encoding]::ASCII)

    $rejection = $null
    try {
        & $boundaryScript -RepositoryRoot $RepositoryRoot -Cargo $cargo
    }
    catch {
        $rejection = $_
    }

    if (-not $rejection) {
        throw 'Expected a feature-gated native dependency to be rejected.'
    }
    if ($rejection.Exception.Message -notmatch 'marknexia-core reaches banned native package windows-core') {
        throw "Expected a feature-gated native dependency rejection, got: $($rejection.Exception.Message)"
    }

    & $boundaryScript -RepositoryRoot $RepositoryRoot -MetadataPath $nativeOnlyFixture

    Write-Host 'Portable dependency boundary all-features test passed.'
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
