#requires -Version 7.0
<#
.SYNOPSIS
Validates a Rust/Win32 SPDX SBOM against the package files and the lock files.

.DESCRIPTION
Checks, independently of Generate-RustSbom.ps1:
1. The document is well-formed JSON, SPDX-2.3, with unique IDs and defined relationship ends.
2. The crate set equals the set reachable from the root crate in cargo metadata over
   normal and build edges (no dev-only crate, no missing crate).
3. Every registry crate has the Cargo.lock SHA-256 and a pkg:cargo purl.
4. The Mermaid and WebView2 components match their lock files.
5. The file inventory equals the files in PackageRoot, and every SHA-256 matches.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$SbomPath,
    [Parameter(Mandatory)][string]$PackageRoot,
    [Parameter(Mandatory)][string]$MetadataPath,
    [Parameter(Mandatory)][ValidateSet('x64', 'ARM64')][string]$Architecture,
    [string]$CargoLockPath = (Join-Path $PSScriptRoot '..\Cargo.lock'),
    [string]$LoaderLockPath = (Join-Path $PSScriptRoot '..\packaging\webview\loader.lock.json'),
    [string]$MermaidLockPath = (Join-Path $PSScriptRoot '..\packaging\webview\mermaid.lock.json'),
    [string]$RootPackage = 'marknexia-win32',
    [string]$ShellFileName = 'marknexia-win32.exe',
    [string]$LoaderFileName = 'WebView2Loader.dll'
)

$ErrorActionPreference = 'Stop'
$failures = [Collections.Generic.List[string]]::new()
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { $failures.Add($Message) } }

$text = Get-Content -LiteralPath $SbomPath -Raw
if (-not (Test-Json -Json $text -ErrorAction SilentlyContinue)) { throw "SBOM is not well-formed JSON: $SbomPath" }
$sbom = $text | ConvertFrom-Json -AsHashtable
Assert-That ($sbom.spdxVersion -ceq 'SPDX-2.3') 'spdxVersion must be SPDX-2.3.'
Assert-That ($sbom.dataLicense -ceq 'CC0-1.0') 'dataLicense must be CC0-1.0.'
Assert-That ($sbom.SPDXID -ceq 'SPDXRef-DOCUMENT') 'Document SPDXID must be SPDXRef-DOCUMENT.'
Assert-That ($sbom.documentNamespace -match '^https://spdx\.org/spdxdocs/marknexia-win32-[0-9a-f]{64}$') 'documentNamespace is missing or malformed.'
# ConvertFrom-Json turns dates into DateTime values, so check the raw text.
Assert-That ($text -match '"created": "\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z"') 'creationInfo.created is malformed.'
$packages = @($sbom.packages)
$files = @($sbom.files)
$ids = @('SPDXRef-DOCUMENT') + @($packages | ForEach-Object SPDXID) + @($files | ForEach-Object SPDXID)
Assert-That (@($ids | Select-Object -Unique).Count -eq $ids.Count) 'SPDX IDs are not unique.'
foreach ($id in $ids) { Assert-That ($id -cmatch '^SPDXRef-[A-Za-z0-9.-]+$') "Invalid SPDX ID: $id" }
foreach ($edge in @($sbom.relationships)) {
    Assert-That ($ids -ccontains $edge.spdxElementId) "Undefined relationship origin: $($edge.spdxElementId)"
    Assert-That ($ids -ccontains $edge.relatedSpdxElement) "Undefined relationship target: $($edge.relatedSpdxElement)"
}
foreach ($package in $packages) {
    foreach ($field in 'name', 'versionInfo', 'downloadLocation', 'licenseConcluded', 'licenseDeclared', 'copyrightText') {
        Assert-That (-not [string]::IsNullOrWhiteSpace($package[$field])) "Package $($package.SPDXID) has no $field."
    }
}

# --- Crates: recompute the expected set from cargo metadata. ------------------
$metadata = Get-Content -LiteralPath $MetadataPath -Raw | ConvertFrom-Json -AsHashtable
$packagesById = @{}; foreach ($p in $metadata.packages) { $packagesById[$p.id] = $p }
$nodesById = @{}; foreach ($n in $metadata.resolve.nodes) { $nodesById[$n.id] = $n }
$root = @($metadata.packages | Where-Object { $_.name -ceq $RootPackage -and $metadata.workspace_members -contains $_.id })
if ($root.Count -ne 1) { throw "Root crate $RootPackage not found exactly once in metadata." }
$expected = [Collections.Generic.HashSet[string]]::new()
$queue = [Collections.Generic.Queue[string]]::new()
[void]$expected.Add($root[0].id); $queue.Enqueue($root[0].id)
while ($queue.Count -gt 0) {
    foreach ($dependency in @($nodesById[$queue.Dequeue()].deps)) {
        if (@($dependency.dep_kinds | Where-Object { $null -eq $_.kind -or $_.kind -eq 'build' }).Count -eq 0) { continue }
        if ($expected.Add($dependency.pkg)) { $queue.Enqueue($dependency.pkg) }
    }
}
$expectedKeys = @($expected | ForEach-Object { "$($packagesById[$_].name)@$($packagesById[$_].version)" } | Sort-Object -CaseSensitive)
$crateEntries = @($packages | Where-Object { $_.SPDXID -like 'SPDXRef-Cargo-*' -or $_.SPDXID -like 'SPDXRef-Workspace-*' })
$actualKeys = @($crateEntries | ForEach-Object { "$($_.name)@$($_.versionInfo)" } | Sort-Object -CaseSensitive)
$missing = @($expectedKeys | Where-Object { $actualKeys -cnotcontains $_ })
$extra = @($actualKeys | Where-Object { $expectedKeys -cnotcontains $_ })
Assert-That ($missing.Count -eq 0) "SBOM is missing crates: $($missing -join ', ')"
Assert-That ($extra.Count -eq 0) "SBOM lists crates outside the shipped graph (dev-only or unrelated): $($extra -join ', ')"
Assert-That (@($crateEntries | Where-Object { $_.SPDXID -like 'SPDXRef-Cargo-*' }).Count -gt 0) 'SBOM lists no registry crates.'

$lockChecksums = @{}
$blocks = [IO.File]::ReadAllText([IO.Path]::GetFullPath($CargoLockPath)) -split '(?m)^\[\[package\]\]\s*$'
foreach ($block in $blocks | Select-Object -Skip 1) {
    $name = [regex]::Match($block, '(?m)^name = "([^"]*)"').Groups[1].Value
    $version = [regex]::Match($block, '(?m)^version = "([^"]*)"').Groups[1].Value
    $checksum = [regex]::Match($block, '(?m)^checksum = "([^"]*)"').Groups[1].Value
    if ($checksum) { $lockChecksums["$name@$version"] = $checksum }
}
foreach ($crate in $crateEntries | Where-Object { $_.SPDXID -like 'SPDXRef-Cargo-*' }) {
    $key = "$($crate.name)@$($crate.versionInfo)"
    $sha = @($crate.checksums | Where-Object algorithm -ceq 'SHA256')
    Assert-That ($sha.Count -eq 1 -and $lockChecksums.ContainsKey($key) -and $sha[0].checksumValue -ceq $lockChecksums[$key]) "Crate $key checksum does not match Cargo.lock."
    Assert-That (@($crate.externalRefs | Where-Object referenceLocator -ceq "pkg:cargo/$key").Count -eq 1) "Crate $key has no pkg:cargo purl."
}

# --- Locked non-Rust components. --------------------------------------------
$mermaidLock = Get-Content -LiteralPath $MermaidLockPath -Raw | ConvertFrom-Json -AsHashtable
$loaderLock = Get-Content -LiteralPath $LoaderLockPath -Raw | ConvertFrom-Json -AsHashtable
$mermaid = @($packages | Where-Object { $_.name -ceq 'mermaid' })
Assert-That ($mermaid.Count -eq 1 -and $mermaid[0].versionInfo -ceq $mermaidLock.version -and
    $mermaid[0].checksums[0].checksumValue -ceq $mermaidLock.served.sha256) 'Mermaid component does not match mermaid.lock.json.'
$webView = @($packages | Where-Object { $_.name -ceq $loaderLock.packageId })
Assert-That ($webView.Count -eq 1 -and $webView[0].versionInfo -ceq $loaderLock.version -and
    $webView[0].checksums[0].checksumValue -ceq $loaderLock.packageSha256) 'WebView2 SDK component does not match loader.lock.json.'

# --- Files: the inventory must equal the package root, hash for hash. ---------------
$rootFull = [IO.Path]::GetFullPath($PackageRoot)
$onDisk = @(Get-ChildItem -LiteralPath $rootFull -File -Recurse | ForEach-Object { './' + [IO.Path]::GetRelativePath($rootFull, $_.FullName).Replace('\', '/') } | Sort-Object -CaseSensitive)
$listed = @($files | ForEach-Object fileName | Sort-Object -CaseSensitive)
Assert-That (($onDisk -join '|') -ceq ($listed -join '|')) "File inventory differs from package root. Disk: $($onDisk -join ', '); SBOM: $($listed -join ', ')"
Assert-That ($listed -ccontains "./$ShellFileName") "SBOM does not list $ShellFileName."
Assert-That ($listed -ccontains "./$LoaderFileName") "SBOM does not list $LoaderFileName."
foreach ($file in $files) {
    $path = Join-Path $rootFull ($file.fileName.Substring(2))
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-That ($file.checksums[0].algorithm -ceq 'SHA256' -and $file.checksums[0].checksumValue -ceq $actual) "SHA-256 mismatch for $($file.fileName)."
}
$loaderFile = @($files | Where-Object fileName -ceq "./$LoaderFileName")
Assert-That ($loaderFile.Count -eq 1 -and $loaderFile[0].checksums[0].checksumValue -ceq $loaderLock.loaders[$Architecture].sha256) "$LoaderFileName is not the pinned $Architecture loader."

if ($failures.Count -gt 0) { throw "SBOM validation failed:`n$($failures -join "`n")" }
Write-Output "SBOM valid: $($crateEntries.Count) crates, $($files.Count) files, every hash matches."
