[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
    [string]$Cargo = (Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe')
)

$ErrorActionPreference = 'Stop'

$manifestPath = Join-Path $RepositoryRoot 'Cargo.toml'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Rust workspace manifest was not found: $manifestPath"
}

if (-not (Test-Path -LiteralPath $Cargo -PathType Leaf)) {
    throw "Cargo executable was not found: $Cargo"
}

$metadataJson = & $Cargo metadata --format-version 1 --manifest-path $manifestPath --locked
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with exit code $LASTEXITCODE"
}

$metadata = $metadataJson | ConvertFrom-Json
$portableCrates = @(
    'marknexia-core',
    'marknexia-files',
    'marknexia-navigation',
    'marknexia-markdown',
    'marknexia-security',
    'marknexia-rendering',
    'marknexia-update'
)
$bannedPackages = @('windows', 'windows-core', 'webview2-com', 'webview2-com-sys')

$violations = foreach ($package in $metadata.packages | Where-Object { $_.name -in $portableCrates }) {
    foreach ($dependency in $package.dependencies | Where-Object { $_.name -in $bannedPackages }) {
        "$($package.name) depends on banned native package $($dependency.name)"
    }
}

if ($violations.Count -gt 0) {
    throw ($violations -join [Environment]::NewLine)
}

Write-Host "Portable dependency boundary passed for $($portableCrates.Count) crates."
