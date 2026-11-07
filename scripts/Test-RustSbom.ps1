#requires -Version 7.0
<#
.SYNOPSIS
Validates a Rust/Win32 SPDX SBOM against the package files, cargo metadata and the lock files.

.DESCRIPTION
Checks, independently of Generate-RustSbom.ps1, and fails closed:
1. Well-formed JSON, SPDX-2.3, unique IDs with known prefixes, defined relationship ends,
   and a DESCRIBES edge from the document to the distribution package.
2. The distribution package: filesAnalyzed=true, a correct packageVerificationCode, and
   CONTAINS every file. No filesAnalyzed=false package CONTAINS a file.
3. The crate set equals the set recomputed from cargo metadata (target graph for runtime, host
   graph for build-time; dev edges never), with the same runtime/build-time scope. Workspace-*
   IDs only for workspace members, Cargo-* IDs only for crates.io crates. The package count is
   the crate count plus the distribution, Mermaid and WebView2 packages.
4. Optionally, the crate set equals `cargo tree -e normal,build --prefix none --format "{p}"`.
5. Every registry crate has the Cargo.lock SHA-256 and a pkg:cargo purl; Mermaid and WebView2
   match their locks (SHA256 algorithm and purl). Every licenseDeclared is a valid expression.
6. The file inventory equals the files in PackageRoot; SHA1 and SHA256 match each file.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$SbomPath,
    [Parameter(Mandatory)][string]$PackageRoot,
    [Parameter(Mandatory)][string]$MetadataPath,
    [Parameter(Mandatory)][ValidateSet('x64', 'ARM64')][string]$Architecture,
    [string]$HostMetadataPath,
    [string]$CargoTreePath,
    [string]$CargoLockPath = (Join-Path $PSScriptRoot '..\Cargo.lock'),
    [string]$LoaderLockPath = (Join-Path $PSScriptRoot '..\packaging\webview\loader.lock.json'),
    [string]$MermaidLockPath = (Join-Path $PSScriptRoot '..\packaging\webview\mermaid.lock.json'),
    [string]$RootPackage = 'marknexia-win32',
    [string]$ShellFileName = 'marknexia-win32.exe',
    [string]$LoaderFileName = 'WebView2Loader.dll',
    # The portable ZIP by default; the setup and MSIX SBOMs name their own artifact.
    [string]$DistributionFileNamePattern = '^marknexia-win32-.+-unsigned-preview\.zip$'
)

$ErrorActionPreference = 'Stop'
$cratesIoSource = 'registry+https://github.com/rust-lang/crates.io-index'
$failures = [Collections.Generic.List[string]]::new()
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { $failures.Add($Message) } }
function Get-Digest([string]$Path, [string]$Algorithm) { (Get-FileHash -LiteralPath $Path -Algorithm $Algorithm).Hash.ToLowerInvariant() }
function Get-Checksum([object]$Element, [string]$Algorithm) {
    $values = @($Element.checksums | Where-Object { $_.algorithm -ceq $Algorithm })
    if ($values.Count -eq 1) { [string]$values[0].checksumValue } else { $null }
}

# SPDX license expression (SPDX 2.3 annex D): idstring ["+"] ["WITH" idstring], AND/OR, parentheses.
function Test-LicenseExpression([string]$Expression) {
    if ($Expression -ceq 'NOASSERTION' -or $Expression -ceq 'NONE') { return $true }
    $tokens = @([regex]::Matches($Expression, '\(|\)|[^\s()]+') | ForEach-Object Value)
    if ($tokens.Count -eq 0 -or ($tokens -join '') -cne ($Expression -replace '\s', '')) { return $false }
    $script:position = 0
    $id = '^(?:DocumentRef-[A-Za-z0-9.-]+:)?(?:LicenseRef-[A-Za-z0-9.-]+|[A-Za-z0-9][A-Za-z0-9.-]*\+?)$'
    $keywords = @('AND', 'OR', 'WITH')
    function Read-Term {
        if ($script:position -ge $tokens.Count) { return $false }
        if ($tokens[$script:position] -ceq '(') {
            $script:position++
            if (-not (Read-Expression)) { return $false }
            if ($script:position -ge $tokens.Count -or $tokens[$script:position] -cne ')') { return $false }
            $script:position++; return $true
        }
        $token = $tokens[$script:position]
        if ($keywords -ccontains $token -or $token -cnotmatch $id) { return $false }
        $script:position++
        if ($script:position -lt $tokens.Count -and $tokens[$script:position] -ceq 'WITH') {
            $script:position++
            if ($script:position -ge $tokens.Count -or $keywords -ccontains $tokens[$script:position] -or $tokens[$script:position] -cnotmatch $id) { return $false }
            $script:position++
        }
        $true
    }
    function Read-Expression {
        if (-not (Read-Term)) { return $false }
        while ($script:position -lt $tokens.Count -and @('AND', 'OR') -ccontains $tokens[$script:position]) {
            $script:position++
            if (-not (Read-Term)) { return $false }
        }
        $true
    }
    (Read-Expression) -and $script:position -eq $tokens.Count
}

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
$relationships = @($sbom.relationships)
$ids = @('SPDXRef-DOCUMENT') + @($packages | ForEach-Object SPDXID) + @($files | ForEach-Object SPDXID)
Assert-That (@($ids | Select-Object -Unique).Count -eq $ids.Count) 'SPDX IDs are not unique.'
foreach ($package in $packages) {
    Assert-That ($package.SPDXID -cmatch '^SPDXRef-(Distribution|Workspace|Cargo|Asset|NuGet)-[A-Za-z0-9.-]+$') "Unknown or invalid package SPDX ID: $($package.SPDXID)"
    foreach ($field in 'name', 'versionInfo', 'downloadLocation', 'licenseConcluded', 'licenseDeclared', 'copyrightText') {
        Assert-That (-not [string]::IsNullOrWhiteSpace($package[$field])) "Package $($package.SPDXID) has no $field."
    }
    Assert-That ($package.filesAnalyzed -is [bool]) "Package $($package.SPDXID) must state filesAnalyzed."
    Assert-That (Test-LicenseExpression ([string]$package.licenseDeclared)) "Package $($package.SPDXID) has an invalid licenseDeclared: $($package.licenseDeclared)"
}
foreach ($file in $files) { Assert-That ($file.SPDXID -cmatch '^SPDXRef-File-[0-9a-f]{64}$') "Unknown or invalid file SPDX ID: $($file.SPDXID)" }
foreach ($edge in $relationships) {
    Assert-That ($ids -ccontains $edge.spdxElementId) "Undefined relationship origin: $($edge.spdxElementId)"
    Assert-That ($ids -ccontains $edge.relatedSpdxElement) "Undefined relationship target: $($edge.relatedSpdxElement)"
}

# --- Distribution package and files (SPDX 2.3 sections 7.8, 7.9, 8.4). ----------
Assert-That (@($relationships | Where-Object { $_.relationshipType -ceq 'DESCRIBES' -and $_.spdxElementId -ceq 'SPDXRef-DOCUMENT' }).Count -ge 1) 'The document has no DESCRIBES relationship.'
$distribution = @($packages | Where-Object { $_.SPDXID -clike 'SPDXRef-Distribution-*' })
Assert-That ($distribution.Count -eq 1) 'Exactly one distribution package is required.'
$fileIds = @($files | ForEach-Object SPDXID)
if ($distribution.Count -eq 1) {
    $zip = $distribution[0]
    Assert-That (@($relationships | Where-Object { $_.spdxElementId -ceq 'SPDXRef-DOCUMENT' -and $_.relationshipType -ceq 'DESCRIBES' -and $_.relatedSpdxElement -ceq $zip.SPDXID }).Count -eq 1) 'The document must DESCRIBE the distribution package.'
    Assert-That ($zip.filesAnalyzed -eq $true) 'The distribution package must have filesAnalyzed=true.'
    Assert-That ([string]$zip.packageFileName -cmatch $DistributionFileNamePattern) "The distribution package must name the unsigned preview artifact ($DistributionFileNamePattern)."
    $contained = @($relationships | Where-Object { $_.spdxElementId -ceq $zip.SPDXID -and $_.relationshipType -ceq 'CONTAINS' } | ForEach-Object relatedSpdxElement)
    Assert-That ((@($contained | Sort-Object) -join '|') -ceq (@($fileIds | Sort-Object) -join '|')) 'The distribution package must CONTAIN exactly the listed files.'
    $sha1Values = [string[]]@($files | ForEach-Object { Get-Checksum $_ 'SHA1' })
    [Array]::Sort($sha1Values, [StringComparer]::Ordinal)
    $expectedCode = [Convert]::ToHexString([Security.Cryptography.SHA1]::HashData([Text.Encoding]::ASCII.GetBytes(($sha1Values -join '')))).ToLowerInvariant()
    Assert-That ($zip.packageVerificationCode.packageVerificationCodeValue -ceq $expectedCode) 'packageVerificationCode does not match the file SHA1 values.'
}
foreach ($package in $packages | Where-Object { $_.filesAnalyzed -ne $true }) {
    Assert-That (-not $package.ContainsKey('packageVerificationCode')) "Package $($package.SPDXID) has a verification code but filesAnalyzed=false."
    $containsFiles = @($relationships | Where-Object { $_.spdxElementId -ceq $package.SPDXID -and $_.relationshipType -ceq 'CONTAINS' -and $fileIds -ccontains $_.relatedSpdxElement })
    Assert-That ($containsFiles.Count -eq 0) "Package $($package.SPDXID) has filesAnalyzed=false but CONTAINS files."
}

# --- Crates: recompute the expected set and scope from cargo metadata. ----------
function Read-Metadata([string]$Path) {
    $value = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json -AsHashtable
    if ($value.version -ne 1 -or -not $value.resolve) { throw "$Path is not cargo metadata with a resolve graph." }
    $value
}
$metadata = Read-Metadata $MetadataPath
$hostMetadata = if ($HostMetadataPath) { Read-Metadata $HostMetadataPath } else { $metadata }
$packagesById = @{}; foreach ($p in @($metadata.packages) + @($hostMetadata.packages)) { $packagesById[$p.id] = $p }
$graphs = @{ target = @{}; host = @{} }
foreach ($n in $metadata.resolve.nodes) { $graphs.target[$n.id] = $n }
foreach ($n in $hostMetadata.resolve.nodes) { $graphs.host[$n.id] = $n }
$workspace = @($metadata.workspace_members)
$root = @($metadata.packages | Where-Object { $_.name -ceq $RootPackage -and $workspace -contains $_.id })
if ($root.Count -ne 1) { throw "Root crate $RootPackage not found exactly once in metadata." }
$scopeById = @{}
$seen = [Collections.Generic.HashSet[string]]::new()
$queue = [Collections.Generic.Queue[object]]::new()
$queue.Enqueue(@($root[0].id, 'target')); [void]$seen.Add("target|$($root[0].id)")
while ($queue.Count -gt 0) {
    $id, $graph = $queue.Dequeue()
    if ($graph -eq 'target') { $scopeById[$id] = 'runtime' } elseif (-not $scopeById.ContainsKey($id)) { $scopeById[$id] = 'build-time' }
    $node = $graphs[$graph][$id]
    if (-not $node) { throw "The $graph resolve graph has no node for $id." }
    foreach ($dependency in @($node.deps)) {
        $target = $packagesById[$dependency.pkg]
        if (-not $target) { throw "Dependency $($dependency.pkg) is missing from packages." }
        $procMacro = @($target.targets | Where-Object { $_.kind -contains 'proc-macro' }).Count -gt 0
        foreach ($kind in @($dependency.dep_kinds | ForEach-Object { if ($null -eq $_.kind) { 'normal' } else { $_.kind } })) {
            $next = switch ($kind) {
                'normal' { if ($graph -eq 'host' -or $procMacro) { 'host' } else { 'target' } }
                'build' { 'host' }
                default { $null }
            }
            if ($next -and $seen.Add("$next|$($dependency.pkg)")) { $queue.Enqueue(@($dependency.pkg, $next)) }
        }
    }
}
$expectedById = @{}
foreach ($id in $scopeById.Keys) { $expectedById["$($packagesById[$id].name)@$($packagesById[$id].version)"] = $id }
$expectedKeys = @($expectedById.Keys | Sort-Object)
$crateEntries = @($packages | Where-Object { $_.SPDXID -clike 'SPDXRef-Cargo-*' -or $_.SPDXID -clike 'SPDXRef-Workspace-*' })
$actualKeys = @($crateEntries | ForEach-Object { "$($_.name)@$($_.versionInfo)" } | Sort-Object)
$missing = @($expectedKeys | Where-Object { $actualKeys -cnotcontains $_ })
$extra = @($actualKeys | Where-Object { $expectedKeys -cnotcontains $_ })
Assert-That ($missing.Count -eq 0) "SBOM is missing crates: $($missing -join ', ')"
Assert-That ($extra.Count -eq 0) "SBOM lists crates outside the shipped graph (dev-only or unrelated): $($extra -join ', ')"
Assert-That ($packages.Count -eq $expectedKeys.Count + 3) "Package count $($packages.Count) is not the crate count $($expectedKeys.Count) plus the distribution, Mermaid and WebView2 packages."
Assert-That (@($crateEntries | Where-Object { $_.SPDXID -clike 'SPDXRef-Cargo-*' }).Count -gt 0) 'SBOM lists no registry crates.'
foreach ($crate in $crateEntries) {
    $key = "$($crate.name)@$($crate.versionInfo)"
    if (-not $expectedById.ContainsKey($key)) { continue }
    $source = $packagesById[$expectedById[$key]]
    if ($crate.SPDXID -clike 'SPDXRef-Workspace-*') {
        Assert-That ($workspace -contains $source.id) "$($crate.SPDXID) is not a workspace member."
    } else {
        Assert-That ($source.source -ceq $cratesIoSource -and $workspace -notcontains $source.id) "$($crate.SPDXID) is not a crates.io crate."
    }
    $scope = $scopeById[$expectedById[$key]]
    Assert-That ([string]$crate.comment -clike "*($scope)*") "Crate $key must be marked ($scope)."
}
if ($CargoTreePath) {
    $treeKeys = @(Get-Content -LiteralPath $CargoTreePath | ForEach-Object { if ($_ -cmatch '^(\S+) v(\S+)') { "$($Matches[1])@$($Matches[2])" } } | Select-Object -Unique | Sort-Object)
    Assert-That ($treeKeys.Count -gt 0) 'cargo tree output lists no crates.'
    $onlyTree = @($treeKeys | Where-Object { $actualKeys -cnotcontains $_ })
    $onlySbom = @($actualKeys | Where-Object { $treeKeys -cnotcontains $_ })
    Assert-That ($onlyTree.Count -eq 0 -and $onlySbom.Count -eq 0) "Crate set differs from cargo tree. Only in cargo tree: $($onlyTree -join ', '); only in SBOM: $($onlySbom -join ', ')"
}

$lockChecksums = @{}
$blocks = [IO.File]::ReadAllText([IO.Path]::GetFullPath($CargoLockPath)) -split '(?m)^\[\[package\]\]\s*$'
foreach ($block in $blocks | Select-Object -Skip 1) {
    $name = [regex]::Match($block, '(?m)^name = "([^"]*)"').Groups[1].Value
    $version = [regex]::Match($block, '(?m)^version = "([^"]*)"').Groups[1].Value
    $checksum = [regex]::Match($block, '(?m)^checksum = "([^"]*)"').Groups[1].Value
    if ($checksum) { $lockChecksums["$name@$version"] = $checksum }
}
foreach ($crate in $crateEntries | Where-Object { $_.SPDXID -clike 'SPDXRef-Cargo-*' }) {
    $key = "$($crate.name)@$($crate.versionInfo)"
    $sha = Get-Checksum $crate 'SHA256'
    Assert-That ($null -ne $sha -and $lockChecksums.ContainsKey($key) -and $sha -ceq $lockChecksums[$key]) "Crate $key checksum does not match Cargo.lock."
    Assert-That (@($crate.externalRefs | Where-Object referenceLocator -ceq "pkg:cargo/$key").Count -eq 1) "Crate $key has no pkg:cargo purl."
}

# --- Locked non-Rust components. --------------------------------------------
$mermaidLock = Get-Content -LiteralPath $MermaidLockPath -Raw | ConvertFrom-Json -AsHashtable
$loaderLock = Get-Content -LiteralPath $LoaderLockPath -Raw | ConvertFrom-Json -AsHashtable
$mermaid = @($packages | Where-Object { $_.SPDXID -clike 'SPDXRef-Asset-*' })
Assert-That ($mermaid.Count -eq 1 -and $mermaid[0].name -ceq 'mermaid' -and $mermaid[0].versionInfo -ceq $mermaidLock.version -and
    (Get-Checksum $mermaid[0] 'SHA256') -ceq $mermaidLock.served.sha256 -and @($mermaid[0].checksums).Count -eq 1 -and
    @($mermaid[0].externalRefs | Where-Object { $_.referenceType -ceq 'purl' -and $_.referenceLocator -ceq "pkg:npm/mermaid@$($mermaidLock.version)" }).Count -eq 1) 'Mermaid component does not match mermaid.lock.json (SHA256 and purl).'
$webView = @($packages | Where-Object { $_.SPDXID -clike 'SPDXRef-NuGet-*' })
Assert-That ($webView.Count -eq 1 -and $webView[0].name -ceq $loaderLock.packageId -and $webView[0].versionInfo -ceq $loaderLock.version -and
    (Get-Checksum $webView[0] 'SHA256') -ceq $loaderLock.packageSha256 -and @($webView[0].checksums).Count -eq 1 -and
    @($webView[0].externalRefs | Where-Object { $_.referenceType -ceq 'purl' -and $_.referenceLocator -ceq "pkg:nuget/$($loaderLock.packageId)@$($loaderLock.version)" }).Count -eq 1) 'WebView2 SDK component does not match loader.lock.json (SHA256 and purl).'

# --- Files: the inventory must equal the package root, hash for hash. ---------------
$rootFull = [IO.Path]::GetFullPath($PackageRoot)
$onDisk = @(Get-ChildItem -LiteralPath $rootFull -File -Recurse | ForEach-Object { './' + [IO.Path]::GetRelativePath($rootFull, $_.FullName).Replace('\', '/') } | Sort-Object)
$listed = @($files | ForEach-Object fileName | Sort-Object)
Assert-That (($onDisk -join '|') -ceq ($listed -join '|')) "File inventory differs from package root. Disk: $($onDisk -join ', '); SBOM: $($listed -join ', ')"
Assert-That ($listed -ccontains "./$ShellFileName") "SBOM does not list $ShellFileName."
Assert-That ($listed -ccontains "./$LoaderFileName") "SBOM does not list $LoaderFileName."
foreach ($file in $files) {
    $path = Join-Path $rootFull ($file.fileName.Substring(2))
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
    Assert-That ((Get-Checksum $file 'SHA1') -ceq (Get-Digest $path 'SHA1')) "SHA1 mismatch for $($file.fileName)."
    Assert-That ((Get-Checksum $file 'SHA256') -ceq (Get-Digest $path 'SHA256')) "SHA-256 mismatch for $($file.fileName)."
}
$loaderFile = @($files | Where-Object fileName -ceq "./$LoaderFileName")
Assert-That ($loaderFile.Count -eq 1 -and (Get-Checksum $loaderFile[0] 'SHA256') -ceq $loaderLock.loaders[$Architecture].sha256) "$LoaderFileName is not the pinned $Architecture loader."

if ($failures.Count -gt 0) { throw "SBOM validation failed:`n$($failures -join "`n")" }
Write-Output "SBOM valid: $($crateEntries.Count) crates, $($files.Count) files, every hash matches."
