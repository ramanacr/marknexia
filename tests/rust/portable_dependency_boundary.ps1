[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
    [string]$Cargo = (Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'),
    [string]$MetadataPath
)

$ErrorActionPreference = 'Stop'

$manifestPath = Join-Path $RepositoryRoot 'Cargo.toml'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Rust workspace manifest was not found: $manifestPath"
}

if ($MetadataPath) {
    if (-not (Test-Path -LiteralPath $MetadataPath -PathType Leaf)) {
        throw "Cargo metadata fixture was not found: $MetadataPath"
    }
    $metadataJson = Get-Content -LiteralPath $MetadataPath -Raw
}
else {
    if (-not (Test-Path -LiteralPath $Cargo -PathType Leaf)) {
        throw "Cargo executable was not found: $Cargo"
    }

    $metadataJson = & $Cargo metadata --format-version 1 --manifest-path $manifestPath --locked --all-features
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE"
    }
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

$packagesById = @{}
foreach ($package in $metadata.packages) {
    $packagesById[$package.id] = $package
}
$nodesById = @{}
foreach ($node in $metadata.resolve.nodes) {
    $nodesById[$node.id] = $node
}

$violations = [System.Collections.Generic.List[string]]::new()
foreach ($root in $metadata.packages | Where-Object { $_.name -in $portableCrates }) {
    $queue = [System.Collections.Generic.Queue[object]]::new()
    $visited = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    $queue.Enqueue([PSCustomObject]@{ PackageId = $root.id; RootName = $root.name })

    while ($queue.Count -gt 0) {
        $current = $queue.Dequeue()
        if (-not $visited.Add($current.PackageId)) {
            continue
        }

        $package = $packagesById[$current.PackageId]
        if (-not $package) {
            throw "Cargo metadata resolve graph references an unknown package '$($current.PackageId)'."
        }
        if ($package.name -in $bannedPackages) {
            $violations.Add("$($current.RootName) reaches banned native package $($package.name)")
            continue
        }

        $node = $nodesById[$current.PackageId]
        if (-not $node) {
            throw "Cargo metadata resolve graph is missing package '$($current.PackageId)'."
        }
        foreach ($dependency in $node.deps) {
            $queue.Enqueue([PSCustomObject]@{ PackageId = $dependency.pkg; RootName = $current.RootName })
        }
    }
}

if ($violations.Count -gt 0) {
    throw ($violations -join [Environment]::NewLine)
}

Write-Host "Portable dependency boundary passed for $($portableCrates.Count) crates."
