#requires -Version 7.0
[CmdletBinding()]
param(
    [string]$OutputPath = "artifacts/marknexia-sbom.spdx.json",
    [string]$AssetsPath = "src/Marknexia.App/obj/project.assets.json",
    [string]$ProductVersion,
    [string[]]$BundledFiles = @("src/Marknexia.Rendering/Assets/mermaid.min.js")
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
function Resolve-RepoPath([string]$Path) {
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}
function Get-ContentDigest([string]$Text) {
    return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($Text))).ToLowerInvariant()
}

$assetsFile = Resolve-RepoPath $AssetsPath
if (-not (Test-Path -LiteralPath $assetsFile -PathType Leaf)) {
    throw "Restore the application first; dependency graph not found: $assetsFile"
}
$assets = Get-Content -LiteralPath $assetsFile -Raw | ConvertFrom-Json -AsHashtable
if (-not $assets.libraries -or -not $assets.targets) { throw "The restore graph is empty or invalid: $assetsFile" }
if (-not $ProductVersion) { $ProductVersion = $assets.project.version }
if ([string]::IsNullOrWhiteSpace($ProductVersion)) { throw "Supply ProductVersion or restore a versioned application." }

# Use only the app's resolved graph. Test projects and unrelated restore outputs
# must not contaminate the product inventory. Include all restored target/RID graphs.
$packages = [Collections.Generic.List[object]]::new()
$packages.Add([ordered]@{
    SPDXID = "SPDXRef-Marknexia"
    name = "Marknexia"
    versionInfo = $ProductVersion
    downloadLocation = "NOASSERTION"
    filesAnalyzed = $false
    licenseConcluded = "NOASSERTION"
    licenseDeclared = "NOASSERTION"
    copyrightText = "NOASSERTION"
})
$idByLibrary = @{}
foreach ($libraryName in @($assets.libraries.Keys | Sort-Object -CaseSensitive)) {
    $library = $assets.libraries[$libraryName]
    if ($library.type -ne "package") { continue }
    $name, $version = $libraryName -split '/', 2
    if (-not $name -or -not $version) { throw "Invalid package identity: $libraryName" }
    $id = "SPDXRef-NuGet-$(Get-ContentDigest $libraryName.ToLowerInvariant())"
    $idByLibrary[$libraryName] = $id
    $package = [ordered]@{
        SPDXID = $id
        name = $name
        versionInfo = $version
        downloadLocation = "NOASSERTION"
        filesAnalyzed = $false
        licenseConcluded = "NOASSERTION"
        licenseDeclared = "NOASSERTION"
        copyrightText = "NOASSERTION"
        externalRefs = @([ordered]@{
            referenceCategory = "PACKAGE-MANAGER"
            referenceType = "purl"
            referenceLocator = "pkg:nuget/$([Uri]::EscapeDataString($name))@$([Uri]::EscapeDataString($version))"
        })
    }
    # Restore records the package archive's SHA-512 in base64; SPDX requires hex.
    # Do not guess a public download location for packages from private feeds.
    if ($library.sha512) {
        $digestBytes = [Convert]::FromBase64String($library.sha512)
        if ($digestBytes.Length -ne 64) { throw "Invalid SHA-512 for $libraryName" }
        $package.checksums = @([ordered]@{
            algorithm = "SHA512"
            checksumValue = [Convert]::ToHexString($digestBytes).ToLowerInvariant()
        })
    }
    $packages.Add($package)
}
if ($idByLibrary.Count -eq 0) { throw "The application restore graph contains no NuGet packages." }

$edgeMap = @{}
function Add-Relationship([string]$From, [string]$Kind, [string]$To) {
    $edgeMap["$From|$Kind|$To"] = [ordered]@{
        spdxElementId = $From
        relationshipType = $Kind
        relatedSpdxElement = $To
    }
}
Add-Relationship "SPDXRef-DOCUMENT" "DESCRIBES" "SPDXRef-Marknexia"
foreach ($target in $assets.targets.Values) {
    $keyByName = @{}
    foreach ($key in $target.Keys) { $keyByName[($key -split '/', 2)[0]] = $key }
    foreach ($key in $target.Keys) {
        $node = $target[$key]
        if ($node.type -eq "package") {
            $from = $idByLibrary[$key]
            if (-not $from) { throw "Package missing from libraries: $key" }
            # The product inventory includes build and runtime packages; detailed
            # edges retain the package-to-package transitive dependency graph.
            Add-Relationship "SPDXRef-Marknexia" "DEPENDS_ON" $from
        } else { $from = "SPDXRef-Marknexia" }
        if ($node.dependencies) {
            foreach ($dependency in $node.dependencies.Keys) {
                $targetKey = $keyByName[$dependency]
                if (-not $targetKey) { throw "Unresolved dependency $dependency in $key" }
                if ($idByLibrary.ContainsKey($targetKey)) {
                    Add-Relationship $from "DEPENDS_ON" $idByLibrary[$targetKey]
                }
            }
        }
    }
}

$files = @($BundledFiles | Sort-Object -Unique | ForEach-Object {
    $path = Resolve-RepoPath $_
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Bundled file not found: $path" }
    $relative = [IO.Path]::GetRelativePath($repoRoot, $path).Replace('\', '/')
    if ($relative.StartsWith('../')) { throw "Bundled file must be inside the repository: $path" }
    $fileId = "SPDXRef-File-$(Get-ContentDigest $relative)"
    Add-Relationship "SPDXRef-Marknexia" "CONTAINS" $fileId
    [ordered]@{
        SPDXID = $fileId
        fileName = "./$relative"
        checksums = @([ordered]@{ algorithm = "SHA256"; checksumValue = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() })
        licenseConcluded = "NOASSERTION"
        licenseInfoInFiles = @("NOASSERTION")
        copyrightText = "NOASSERTION"
    }
})
$created = if ($env:SOURCE_DATE_EPOCH) {
    [DateTimeOffset]::FromUnixTimeSeconds([long]$env:SOURCE_DATE_EPOCH)
} else { [DateTimeOffset]::UtcNow }
$sbom = [ordered]@{
    spdxVersion = "SPDX-2.3"
    dataLicense = "CC0-1.0"
    SPDXID = "SPDXRef-DOCUMENT"
    name = "Marknexia-$ProductVersion"
    creationInfo = [ordered]@{
        created = $created.ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        creators = @("Tool: Marknexia-SBOM-2.0")
    }
    comment = "Application restore inventory across all restored frameworks/RIDs, including build dependencies, plus listed bundled files. Not a file inventory of the installed Windows runtime. Licenses and download origins are not inferred."
    packages = $packages.ToArray()
    files = $files
    relationships = @($edgeMap.Keys | Sort-Object -CaseSensitive | ForEach-Object { $edgeMap[$_] })
}
$digest = Get-ContentDigest ($sbom | ConvertTo-Json -Depth 20 -Compress)
$sbom.documentNamespace = "https://spdx.org/spdxdocs/marknexia-$digest"
$outputFile = Resolve-RepoPath $OutputPath
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $outputFile) | Out-Null
$sbom | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $outputFile -Encoding utf8NoBOM
Write-Output "SBOM written to $outputFile ($($idByLibrary.Count) NuGet packages, $($files.Count) bundled files)"
