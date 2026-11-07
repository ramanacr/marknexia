#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$FirstRoot,
    [Parameter(Mandatory)][string]$SecondRoot,
    [Parameter(Mandatory)][string]$OutputPath,
    [string[]]$RequiredRelativePaths = @('marknexia-win32.exe')
)

$ErrorActionPreference = 'Stop'
$firstFull = [IO.Path]::GetFullPath($FirstRoot).TrimEnd([IO.Path]::DirectorySeparatorChar)
$secondFull = [IO.Path]::GetFullPath($SecondRoot).TrimEnd([IO.Path]::DirectorySeparatorChar)
$outputFull = [IO.Path]::GetFullPath($OutputPath)
foreach ($root in @($firstFull, $secondFull)) {
    if (-not (Test-Path -LiteralPath $root -PathType Container)) { throw "Build root not found: $root" }
    if ($outputFull.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'OutputPath must be outside both build roots.'
    }
}
if ($firstFull.Equals($secondFull, [StringComparison]::OrdinalIgnoreCase)) { throw 'Build roots must be distinct.' }
if ($RequiredRelativePaths.Count -eq 0) { throw 'At least one required artifact is needed.' }

$files = foreach ($relativePath in $RequiredRelativePaths) {
    if ([IO.Path]::IsPathRooted($relativePath) -or $relativePath -match '(^|[\\/])\.\.([\\/]|$)') {
        throw "Artifact path must remain relative to each build root: $relativePath"
    }
    $firstPath = [IO.Path]::GetFullPath((Join-Path $firstFull $relativePath))
    $secondPath = [IO.Path]::GetFullPath((Join-Path $secondFull $relativePath))
    if (-not (Test-Path -LiteralPath $firstPath -PathType Leaf) -or -not (Test-Path -LiteralPath $secondPath -PathType Leaf)) {
        throw "Required artifact is missing from one of the builds: $relativePath"
    }
    $firstHash = (Get-FileHash -LiteralPath $firstPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $secondHash = (Get-FileHash -LiteralPath $secondPath -Algorithm SHA256).Hash.ToLowerInvariant()
    [ordered]@{
        path = $relativePath.Replace('\', '/')
        firstSha256 = $firstHash
        secondSha256 = $secondHash
        equal = $firstHash -eq $secondHash
    }
}
$result = [ordered]@{
    schemaVersion = 'rust-reproducibility-v1'
    reproducible = @($files | Where-Object { -not $_.equal }).Count -eq 0
    comparison = 'unsigned-before-signing'
    files = @($files)
}
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($outputFull)) | Out-Null
$result | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $outputFull -Encoding utf8NoBOM
if (-not $result.reproducible) { throw "Unsigned build artifacts differ; see $outputFull" }
Write-Host "Unsigned Rust build hashes match: $outputFull"
