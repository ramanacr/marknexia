#requires -Version 7.0
<#
.SYNOPSIS
Writes the SPDX 2.3 SBOM and THIRD-PARTY-NOTICES.txt for one Rust/Win32 package root.

.DESCRIPTION
The inventory comes from `cargo metadata --format-version 1 --locked --filter-platform <target>`
(MetadataPath), the same for the build host (HostMetadataPath; defaults to MetadataPath for a
native build) and Cargo.lock. Starting from the shell crate, the generator follows normal
edges in the target graph (runtime). A build edge, or a normal edge into a proc-macro, moves to
the host graph; everything reached there is build-time, as `cargo tree -e normal,build` shows
it. Dev-dependency edges are never followed. Each registry crate carries the Cargo.lock
SHA-256 of its .crate archive. The embedded Mermaid runtime and the WebView2 SDK package
come from their lock files.

The document DESCRIBES one distribution package, the portable ZIP. It has filesAnalyzed=true,
a packageVerificationCode, and CONTAINS every file in the package root; each file has SHA1
and SHA256 checksums (SPDX 2.3 sections 7.8, 7.9 and 8.4).

The package root must already hold the shell and loader files. The notices file is written
into the package root first, so the SBOM also records its hash. The SBOM must be written
outside the package root. Output is byte-identical for identical inputs when
SOURCE_DATE_EPOCH is set. Licenses are copied from crate metadata, not concluded.
This script never builds, signs or archives anything.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$MetadataPath,
    [Parameter(Mandatory)][string]$PackageRoot,
    [Parameter(Mandatory)][ValidateSet('x64', 'ARM64')][string]$Architecture,
    [Parameter(Mandatory)][string]$OutputPath,
    [string]$HostMetadataPath,
    [string]$DistributionFileName,
    # Wording only: the portable ZIP keeps the defaults; the setup and MSIX name themselves.
    [string]$DistributionDescription = 'portable preview ZIP',
    [string]$DistributionKind = 'ZIP',
    [string]$CargoLockPath = (Join-Path $PSScriptRoot '..\Cargo.lock'),
    [string]$LoaderLockPath = (Join-Path $PSScriptRoot '..\packaging\webview\loader.lock.json'),
    [string]$MermaidLockPath = (Join-Path $PSScriptRoot '..\packaging\webview\mermaid.lock.json'),
    [string]$ProductLicensePath = (Join-Path $PSScriptRoot '..\LICENSE'),
    [string]$LoaderLicensePath,
    [string]$RootPackage = 'marknexia-win32',
    [string]$ShellFileName = 'marknexia-win32.exe',
    [string]$LoaderFileName = 'WebView2Loader.dll',
    [string]$NoticesFileName = 'THIRD-PARTY-NOTICES.txt',
    [string]$ProductVersion,
    [string]$SourceCommit = 'NOASSERTION'
)

$ErrorActionPreference = 'Stop'
$cratesIoSource = 'registry+https://github.com/rust-lang/crates.io-index'
$utf8 = [Text.UTF8Encoding]::new($false)

# Ordinal sort, independent of the runner's culture, so output is byte-stable.
function Select-Ordinal([object[]]$Items, [scriptblock]$Key = { $_ }) {
    if (-not $Items -or $Items.Count -eq 0) { return , @() }
    $keys = [string[]]@($Items | ForEach-Object -Process $Key | ForEach-Object { [string]$_ })
    if ($keys.Count -ne $Items.Count) { throw 'Sort key must yield exactly one value per item.' }
    $sorted = [object[]]$Items.Clone()
    # Cast explicitly: the generic overload binds a converted copy of $sorted and leaves it unsorted.
    [Array]::Sort([Array]$keys, [Array]$sorted, [Collections.IComparer][StringComparer]::Ordinal)
    , $sorted
}
function Get-TextDigest([string]$Text) {
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($utf8.GetBytes($Text))).ToLowerInvariant()
}
function Get-FileDigest([string]$Path, [string]$Algorithm = 'SHA256') {
    (Get-FileHash -LiteralPath $Path -Algorithm $Algorithm).Hash.ToLowerInvariant()
}
function Get-FileDigestText([string]$Text) {
    [Convert]::ToHexString([Security.Cryptography.SHA1]::HashData([Text.Encoding]::ASCII.GetBytes($Text))).ToLowerInvariant()
}
function Get-SpdxId([string]$Prefix, [string]$Name, [string]$Version) {
    $safe = ("$Name-$Version" -replace '[^A-Za-z0-9.-]', '-')
    "SPDXRef-$Prefix-$safe"
}
function ConvertTo-LfText([string]$Text) {
    ($Text -replace "`r`n", "`n" -replace "`r", "`n").TrimEnd()
}
function ConvertTo-SpdxLicense([object]$License) {
    if ([string]::IsNullOrWhiteSpace($License)) { return 'NOASSERTION' }
    # Cargo still accepts the legacy "MIT/Apache-2.0" form, which SPDX does not.
    $expression = (($License -split '\s*/\s*') -join ' OR ').Trim()
    if ($expression -notmatch '^[A-Za-z0-9.+\-() ]+$') { throw "Unsupported license expression: $License" }
    $expression
}

# --- Inputs ----------------------------------------------------------------
$packageRootFull = [IO.Path]::GetFullPath($PackageRoot).TrimEnd([IO.Path]::DirectorySeparatorChar)
$outputFull = [IO.Path]::GetFullPath($OutputPath)
if (-not (Test-Path -LiteralPath $packageRootFull -PathType Container)) { throw "Package root not found: $packageRootFull" }
if ($outputFull.StartsWith($packageRootFull + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'OutputPath must be outside PackageRoot; the SBOM describes the package, it is not part of it.'
}
foreach ($required in @($ShellFileName, $LoaderFileName)) {
    if (-not (Test-Path -LiteralPath (Join-Path $packageRootFull $required) -PathType Leaf)) {
        throw "Package root is missing $required"
    }
}
if (@(Get-ChildItem -LiteralPath $packageRootFull -Directory).Count -gt 0) { throw 'Package root must be flat.' }

function Read-Metadata([string]$Path) {
    $value = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json -AsHashtable
    if ($value.version -ne 1 -or -not $value.resolve -or -not $value.packages) {
        throw "$Path is not ``cargo metadata --format-version 1`` output with a resolve graph."
    }
    $value
}
$metadata = Read-Metadata $MetadataPath
$hostMetadata = if ($HostMetadataPath) { Read-Metadata $HostMetadataPath } else { $metadata }
$loaderLock = Get-Content -LiteralPath $LoaderLockPath -Raw | ConvertFrom-Json -AsHashtable
if ($loaderLock.schemaVersion -ne 'webview2-loader-lock-v1' -or -not $loaderLock.loaders.ContainsKey($Architecture)) {
    throw 'WebView2 loader lock does not cover the requested architecture.'
}
$mermaidLock = Get-Content -LiteralPath $MermaidLockPath -Raw | ConvertFrom-Json -AsHashtable
if ($mermaidLock.schemaVersion -ne 'bundled-runtime-lock-v1' -or $mermaidLock.package -ne 'mermaid') {
    throw 'Mermaid lock has an unexpected schema.'
}
$loaderDigest = Get-FileDigest (Join-Path $packageRootFull $LoaderFileName)
if ($loaderDigest -ne $loaderLock.loaders[$Architecture].sha256) {
    throw "Staged $LoaderFileName does not match the pinned $Architecture loader SHA-256."
}

# Cargo.lock: [[package]] blocks with name, version, source and checksum.
$lockChecksums = @{}
$blocks = [IO.File]::ReadAllText([IO.Path]::GetFullPath($CargoLockPath)) -split '(?m)^\[\[package\]\]\s*$'
foreach ($block in $blocks | Select-Object -Skip 1) {
    $fields = @{}
    foreach ($match in [regex]::Matches($block, '(?m)^(name|version|checksum) = "([^"]*)"\s*$')) {
        $fields[$match.Groups[1].Value] = $match.Groups[2].Value
    }
    if ($fields.name -and $fields.version -and $fields.checksum) {
        $lockChecksums["$($fields.name)@$($fields.version)"] = $fields.checksum
    }
}

# --- Dependency graph ------------------------------------------------------
$packagesById = @{}
foreach ($package in @($metadata.packages) + @($hostMetadata.packages)) { $packagesById[$package.id] = $package }
$nodesByGraph = @{ target = @{}; host = @{} }
foreach ($node in $metadata.resolve.nodes) { $nodesByGraph.target[$node.id] = $node }
foreach ($node in $hostMetadata.resolve.nodes) { $nodesByGraph.host[$node.id] = $node }
$workspaceMembers = [Collections.Generic.HashSet[string]]::new([string[]]@($metadata.workspace_members))
$rootIds = @($metadata.packages | Where-Object { $_.name -ceq $RootPackage -and $workspaceMembers.Contains($_.id) } | ForEach-Object id)
if ($rootIds.Count -ne 1) { throw "Root workspace package $RootPackage not found exactly once." }
$rootId = $rootIds[0]
if (-not $ProductVersion) { $ProductVersion = $packagesById[$rootId].version }
if (-not $DistributionFileName) { $DistributionFileName = "marknexia-win32-$ProductVersion-$($Architecture.ToLowerInvariant())-unsigned-preview.zip" }

function Get-EdgeKinds([hashtable]$Dependency) {
    @($Dependency.dep_kinds | ForEach-Object { if ($null -eq $_.kind) { 'normal' } else { [string]$_.kind } })
}
function Test-ProcMacro([hashtable]$Package) {
    @($Package.targets | Where-Object { $_.kind -contains 'proc-macro' }).Count -gt 0
}
# Visits (crate, graph) states. Target graph: normal edges into non-proc-macro crates (runtime).
# Build edges and proc-macros move to the host graph, where normal and build edges stay (build-time).
# Dev edges are never followed.
$runtimeIds = [Collections.Generic.HashSet[string]]::new()
$shippedIds = [Collections.Generic.HashSet[string]]::new()
$visited = [Collections.Generic.HashSet[string]]::new()
$queue = [Collections.Generic.Queue[object]]::new()
$queue.Enqueue(@($rootId, 'target')); [void]$visited.Add("target|$rootId")
$crateEdges = [Collections.Generic.List[object]]::new()
while ($queue.Count -gt 0) {
    $id, $graph = $queue.Dequeue()
    [void]$shippedIds.Add($id)
    if ($graph -eq 'target') { [void]$runtimeIds.Add($id) }
    $node = $nodesByGraph[$graph][$id]
    if (-not $node) { throw "The $graph resolve graph has no node for $id." }
    foreach ($dependency in @($node.deps)) {
        $kinds = Get-EdgeKinds $dependency
        $target = $packagesById[$dependency.pkg]
        if (-not $target) { throw "Dependency $($dependency.pkg) is missing from packages." }
        $next = [Collections.Generic.List[string]]::new()
        if ($kinds -contains 'normal') {
            $next.Add($(if ($graph -eq 'host' -or (Test-ProcMacro $target)) { 'host' } else { 'target' }))
            $crateEdges.Add(@($id, 'DEPENDS_ON', $dependency.pkg))
        }
        if ($kinds -contains 'build') {
            $next.Add('host')
            $crateEdges.Add(@($dependency.pkg, 'BUILD_DEPENDENCY_OF', $id))
        }
        foreach ($nextGraph in $next) {
            if ($visited.Add("$nextGraph|$($dependency.pkg)")) { $queue.Enqueue(@($dependency.pkg, $nextGraph)) }
        }
    }
}
$crates = Select-Ordinal @($shippedIds | ForEach-Object { $packagesById[$_] }) { "$($_.name)@$($_.version)" }
$crateIdByPackageId = @{}
$spdxPackages = [Collections.Generic.List[object]]::new()
$noticeEntries = [Collections.Generic.List[object]]::new()
foreach ($crate in $crates) {
    $isWorkspace = $workspaceMembers.Contains($crate.id)
    $scope = if ($runtimeIds.Contains($crate.id)) { 'runtime' } else { 'build-time' }
    $prefix = if ($isWorkspace) { 'Workspace' } else { 'Cargo' }
    $spdxId = Get-SpdxId $prefix $crate.name $crate.version
    if ($crateIdByPackageId.Values -contains $spdxId) { throw "SPDX ID collision: $spdxId" }
    $crateIdByPackageId[$crate.id] = $spdxId
    $license = ConvertTo-SpdxLicense $crate.license
    $entry = [ordered]@{
        SPDXID = $spdxId
        name = $crate.name
        versionInfo = $crate.version
        downloadLocation = 'NOASSERTION'
        filesAnalyzed = $false
        licenseConcluded = 'NOASSERTION'
        licenseDeclared = $license
        copyrightText = 'NOASSERTION'
    }
    if ($isWorkspace) {
        if ($null -ne $crate.source) { throw "Workspace member $($crate.name) has a registry source." }
        $entry.comment = "Marknexia workspace crate ($scope); compiled into $ShellFileName, whose file checksum covers it."
    } else {
        if ($crate.source -cne $cratesIoSource) {
            throw "Crate $($crate.name) $($crate.version) comes from $($crate.source); only crates.io is allowed (deny.toml)."
        }
        $checksum = $lockChecksums["$($crate.name)@$($crate.version)"]
        if ($checksum -notmatch '^[0-9a-f]{64}$') { throw "Cargo.lock has no SHA-256 checksum for $($crate.name) $($crate.version)." }
        $entry.downloadLocation = "https://crates.io/api/v1/crates/$($crate.name)/$($crate.version)/download"
        $entry.checksums = @([ordered]@{ algorithm = 'SHA256'; checksumValue = $checksum })
        $entry.externalRefs = @([ordered]@{
            referenceCategory = 'PACKAGE-MANAGER'
            referenceType = 'purl'
            referenceLocator = "pkg:cargo/$($crate.name)@$($crate.version)"
        })
        $entry.comment = "Rust crate ($scope). Checksum: Cargo.lock SHA-256 of the .crate archive."
        $noticeEntries.Add([ordered]@{ crate = $crate; license = $license; scope = $scope })
    }
    $spdxPackages.Add($entry)
}
$rootSpdxId = $crateIdByPackageId[$rootId]
$spdxPackages | Where-Object { $_.SPDXID -eq $rootSpdxId } | ForEach-Object {
    $_.comment = "Marknexia Rust/Win32 shell crate (runtime) for $Architecture; unsigned preview built from source commit $SourceCommit."
}

$mermaidId = Get-SpdxId 'Asset' 'mermaid' $mermaidLock.version
$spdxPackages.Add([ordered]@{
    SPDXID = $mermaidId
    name = 'mermaid'
    versionInfo = $mermaidLock.version
    downloadLocation = 'NOASSERTION'
    filesAnalyzed = $false
    licenseConcluded = 'NOASSERTION'
    licenseDeclared = (ConvertTo-SpdxLicense $mermaidLock.license)
    copyrightText = 'NOASSERTION'
    checksums = @([ordered]@{ algorithm = 'SHA256'; checksumValue = $mermaidLock.served.sha256 })
    externalRefs = @([ordered]@{ referenceCategory = 'PACKAGE-MANAGER'; referenceType = 'purl'; referenceLocator = "pkg:npm/mermaid@$($mermaidLock.version)" })
    comment = "Embedded in $ShellFileName in its CRLF form ($($mermaidLock.served.bytes) bytes; the checksum is of those bytes) from $($mermaidLock.source) at commit $($mermaidLock.sourceCommit); served from $($mermaidLock.servedUri)."
})
$webViewId = Get-SpdxId 'NuGet' $loaderLock.packageId $loaderLock.version
$spdxPackages.Add([ordered]@{
    SPDXID = $webViewId
    name = $loaderLock.packageId
    versionInfo = $loaderLock.version
    downloadLocation = $loaderLock.source
    filesAnalyzed = $false
    licenseConcluded = 'NOASSERTION'
    licenseDeclared = 'NOASSERTION'
    copyrightText = 'NOASSERTION'
    checksums = @([ordered]@{ algorithm = 'SHA256'; checksumValue = $loaderLock.packageSha256 })
    externalRefs = @([ordered]@{ referenceCategory = 'PACKAGE-MANAGER'; referenceType = 'purl'; referenceLocator = "pkg:nuget/$($loaderLock.packageId)@$($loaderLock.version)" })
    comment = "Microsoft WebView2 SDK package; only $($loaderLock.loaders[$Architecture].entry) is shipped, as $LoaderFileName."
})

# --- Notices (written before the file inventory, so the SBOM hashes it) ----------
$rule = '=' * 80
$notice = [Text.StringBuilder]::new()
[void]$notice.Append("THIRD-PARTY NOTICES`n`nMarknexia Rust/Win32 shell $ProductVersion ($Architecture), unsigned preview.`n")
[void]$notice.Append("This file lists the third-party components in this package and their license texts.`n")
[void]$notice.Append("License identifiers are declared by each component; they are copied, not concluded.`n")
[void]$notice.Append("`n$rule`nMarknexia $ProductVersion`nLicense: $(ConvertTo-SpdxLicense $packagesById[$rootId].license)`n$rule`n")
[void]$notice.Append((ConvertTo-LfText ([IO.File]::ReadAllText([IO.Path]::GetFullPath($ProductLicensePath)))) + "`n")
$licenseNamePattern = '^(?i)(licen[cs]e|copying|notice|copyright|unlicense)([-._ ].*)?$'
foreach ($item in $noticeEntries) {
    $crate = $item.crate
    [void]$notice.Append("`n$rule`n$($crate.name) $($crate.version) (Rust crate, $($item.scope))`n")
    [void]$notice.Append("License: $($item.license)`nSource: https://crates.io/crates/$($crate.name)/$($crate.version)`n")
    if ($crate.repository) { [void]$notice.Append("Repository: $($crate.repository)`n") }
    [void]$notice.Append("$rule`n")
    $crateDirectory = Split-Path -Parent ([IO.Path]::GetFullPath($crate.manifest_path))
    $licenseFiles = [Collections.Generic.SortedSet[string]]::new([StringComparer]::Ordinal)
    if (Test-Path -LiteralPath $crateDirectory -PathType Container) {
        Get-ChildItem -LiteralPath $crateDirectory -File | Where-Object { $_.Name -match $licenseNamePattern } |
            ForEach-Object { [void]$licenseFiles.Add($_.Name) }
        if ($crate.license_file) {
            $declared = [IO.Path]::GetFullPath((Join-Path $crateDirectory $crate.license_file))
            if ($declared.StartsWith($crateDirectory + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -and
                (Test-Path -LiteralPath $declared -PathType Leaf)) {
                [void]$licenseFiles.Add([IO.Path]::GetRelativePath($crateDirectory, $declared).Replace('\', '/'))
            }
        }
    }
    if ($licenseFiles.Count -eq 0 -and $item.license -eq 'NOASSERTION') {
        [void]$notice.Append("No license is declared and the crate source carries no license file. See the source above.`n")
    } elseif ($licenseFiles.Count -eq 0) {
        $links = @(($item.license -split '\s+(?:OR|AND|WITH)\s+|[()]') | Where-Object { $_ -match '^[A-Za-z0-9.+-]+$' -and $_ -ne 'NOASSERTION' } |
            Select-Object -Unique | Sort-Object | ForEach-Object { "https://spdx.org/licenses/$($_.TrimEnd('+')).html" })
        [void]$notice.Append("The crate source carries no license file. License text: $($links -join ', ')`n")
    }
    foreach ($name in $licenseFiles) {
        [void]$notice.Append("--- $name ---`n")
        [void]$notice.Append((ConvertTo-LfText ([IO.File]::ReadAllText((Join-Path $crateDirectory $name)))) + "`n")
    }
}
[void]$notice.Append("`n$rule`nmermaid $($mermaidLock.version) (JavaScript, embedded in $ShellFileName)`n")
[void]$notice.Append("License: $(ConvertTo-SpdxLicense $mermaidLock.license)`nSource: https://github.com/mermaid-js/mermaid/tree/v$($mermaidLock.version)`n$rule`n")
[void]$notice.Append("License text: https://github.com/mermaid-js/mermaid/blob/v$($mermaidLock.version)/LICENSE`n")
[void]$notice.Append("The bundled file keeps the license comments of the libraries that Mermaid includes.`n")
[void]$notice.Append("`n$rule`n$($loaderLock.packageId) $($loaderLock.version) ($LoaderFileName)`n")
[void]$notice.Append("Source: https://www.nuget.org/packages/$($loaderLock.packageId)/$($loaderLock.version)`n$rule`n")
if ($LoaderLicensePath) {
    [void]$notice.Append((ConvertTo-LfText ([IO.File]::ReadAllText([IO.Path]::GetFullPath($LoaderLicensePath)))) + "`n")
} else {
    [void]$notice.Append("License text: see the license in the NuGet package above.`n")
}
$noticesPath = Join-Path $packageRootFull $NoticesFileName
[IO.File]::WriteAllText($noticesPath, $notice.ToString(), $utf8)

# --- Files and the distribution package --------------------------------------
$edgeMap = @{}
function Add-Relationship([string]$From, [string]$Kind, [string]$To) {
    $edgeMap["$From|$Kind|$To"] = [ordered]@{ spdxElementId = $From; relationshipType = $Kind; relatedSpdxElement = $To }
}
$distributionId = 'SPDXRef-Distribution-' + ($DistributionFileName -replace '[^A-Za-z0-9.-]', '-')
Add-Relationship 'SPDXRef-DOCUMENT' 'DESCRIBES' $distributionId
$fileSha1 = [Collections.Generic.List[string]]::new()
$files = @((Select-Ordinal @(Get-ChildItem -LiteralPath $packageRootFull -File) { $_.Name }) | ForEach-Object {
    $fileId = "SPDXRef-File-$(Get-TextDigest $_.Name)"
    Add-Relationship $distributionId 'CONTAINS' $fileId
    $isBinary = $_.Name -ceq $ShellFileName -or $_.Name -ceq $LoaderFileName
    if ($_.Name -ceq $ShellFileName) { Add-Relationship $fileId 'CONTAINS' $mermaidId; Add-Relationship $fileId 'GENERATED_FROM' $rootSpdxId }
    if ($_.Name -ceq $LoaderFileName) { Add-Relationship $fileId 'GENERATED_FROM' $webViewId }
    $sha1 = Get-FileDigest $_.FullName 'SHA1'
    $fileSha1.Add($sha1)
    [ordered]@{
        SPDXID = $fileId
        fileName = "./$($_.Name)"
        fileTypes = @($(if ($isBinary) { 'BINARY' } else { 'TEXT' }))
        checksums = @(
            [ordered]@{ algorithm = 'SHA1'; checksumValue = $sha1 },
            [ordered]@{ algorithm = 'SHA256'; checksumValue = (Get-FileDigest $_.FullName) })
        licenseConcluded = 'NOASSERTION'
        licenseInfoInFiles = @('NOASSERTION')
        copyrightText = 'NOASSERTION'
    }
})
# SPDX 2.3 section 7.9: SHA1 over the ascending, concatenated SHA1 values of every file.
$verificationCode = Get-FileDigestText ((Select-Ordinal $fileSha1.ToArray()) -join '')
$spdxPackages.Insert(0, [ordered]@{
    SPDXID = $distributionId
    name = [IO.Path]::GetFileNameWithoutExtension($DistributionFileName)
    versionInfo = $ProductVersion
    packageFileName = $DistributionFileName
    downloadLocation = 'NOASSERTION'
    filesAnalyzed = $true
    packageVerificationCode = [ordered]@{ packageVerificationCodeValue = $verificationCode }
    primaryPackagePurpose = 'ARCHIVE'
    licenseConcluded = 'NOASSERTION'
    licenseInfoFromFiles = @('NOASSERTION')
    licenseDeclared = 'NOASSERTION'
    copyrightText = 'NOASSERTION'
    comment = "Unsigned, unpublished $DistributionDescription for $Architecture (CI evidence only, never released)."
})
Add-Relationship $rootSpdxId 'DEPENDS_ON' $webViewId
foreach ($edge in $crateEdges) {
    if ($crateIdByPackageId.ContainsKey($edge[0]) -and $crateIdByPackageId.ContainsKey($edge[2])) {
        Add-Relationship $crateIdByPackageId[$edge[0]] $edge[1] $crateIdByPackageId[$edge[2]]
    }
}
$created = if ($env:SOURCE_DATE_EPOCH) { [DateTimeOffset]::FromUnixTimeSeconds([long]$env:SOURCE_DATE_EPOCH) } else { [DateTimeOffset]::UtcNow }
$sbom = [ordered]@{
    spdxVersion = 'SPDX-2.3'
    dataLicense = 'CC0-1.0'
    SPDXID = 'SPDXRef-DOCUMENT'
    name = "marknexia-win32-$ProductVersion-$Architecture-unsigned-preview"
    creationInfo = [ordered]@{
        created = $created.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        creators = @('Tool: Marknexia-Rust-SBOM-1.0')
    }
    comment = "Unsigned, unpublished preview package for $Architecture. Crates: cargo metadata resolve graphs (target for runtime, host for build-time) from $RootPackage over normal and build edges, dev-dependencies excluded; the scope is in each package comment. Files: every file in the $DistributionKind with SHA1 and SHA256. Licenses are declared by each component and not concluded."
    packages = $spdxPackages.ToArray()
    files = $files
    relationships = @((Select-Ordinal @($edgeMap.Keys)) | ForEach-Object { $edgeMap[$_] })
}
$digest = Get-TextDigest ($sbom | ConvertTo-Json -Depth 20 -Compress)
$sbom.documentNamespace = "https://spdx.org/spdxdocs/marknexia-win32-$digest"
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($outputFull)) | Out-Null
[IO.File]::WriteAllText($outputFull, ($sbom | ConvertTo-Json -Depth 20) + "`n", $utf8)
$registryCount = $noticeEntries.Count
Write-Output "SBOM written to $outputFull ($registryCount registry crates, $($crates.Count - $registryCount) workspace crates, $($files.Count) files); notices: $noticesPath"
