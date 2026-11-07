#requires -Version 7.0
<#
.SYNOPSIS
Finds and checks the payload container (MNXSETUP-PAYLD-1) inside a built setup executable,
independently of the Rust code, and compares it with the expected payload files.

.DESCRIPTION
Reads the file as bytes (it never runs it). The container format is documented in
crates/marknexia-setup/src/container.rs:

  magic "MNXSETUP-PAYLD-1" | u32 count | count x (u16 name length, name, u64 size, 32-byte SHA-256)
  | file bytes | 32-byte SHA-256 of everything before it

Every occurrence of the magic is tried; exactly one must parse with a matching trailer
(the magic constant in the parser's code does not). Then:
1. the entry names equal ExpectedNames exactly and are in ordinal order;
2. every entry's SHA-256 matches its bytes and, with ExpectedRoot, the file of the same name;
3. with ExtractTo (CI only, because entries include executables), each entry is written there.
Outputs the container summary as an object.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$SetupPath,
    [string]$ExpectedRoot,
    [string[]]$ExpectedNames = @('README.txt', 'THIRD-PARTY-NOTICES.txt', 'WebView2Loader.dll', 'marknexia-sbom.spdx.json', 'marknexia-win32.exe'),
    [string]$ExtractTo
)

$ErrorActionPreference = 'Stop'
if ($ExtractTo -and $env:GITHUB_ACTIONS -ne 'true') {
    throw 'ExtractTo writes the payload (executables) to disk; it runs only on GitHub Actions runners.'
}

$magicText = 'MNXSETUP-PAYLD-1'
$latin1 = [Text.Encoding]::Latin1
$bytes = [IO.File]::ReadAllBytes([IO.Path]::GetFullPath($SetupPath))
$text = $latin1.GetString($bytes)

function Get-Sha256Hex([byte[]]$Data, [int]$Offset, [int]$Count) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { [Convert]::ToHexString($sha.ComputeHash($Data, $Offset, $Count)).ToLowerInvariant() } finally { $sha.Dispose() }
}

function Read-Container([int]$Start) {
    $position = $Start + 16
    $end = $bytes.Length
    if ($position + 4 -gt $end) { return $null }
    $count = [BitConverter]::ToUInt32($bytes, $position); $position += 4
    if ($count -lt 1 -or $count -gt 16) { return $null }
    $headers = [Collections.Generic.List[object]]::new()
    for ($i = 0; $i -lt $count; $i++) {
        if ($position + 2 -gt $end) { return $null }
        $length = [BitConverter]::ToUInt16($bytes, $position); $position += 2
        if ($length -lt 1 -or $length -gt 64 -or $position + $length + 40 -gt $end) { return $null }
        $name = $latin1.GetString($bytes, $position, $length); $position += $length
        $size = [BitConverter]::ToUInt64($bytes, $position); $position += 8
        $hash = [Convert]::ToHexString($bytes, $position, 32).ToLowerInvariant(); $position += 32
        $headers.Add([pscustomobject]@{ name = $name; size = $size; sha256 = $hash })
    }
    $dataStart = $position
    $total = [uint64]0
    foreach ($header in $headers) { $total += $header.size }
    if ($dataStart + $total + 32 -gt [uint64]$end) { return $null }
    $trailerAt = [int]($dataStart + $total)
    $expectedTrailer = [Convert]::ToHexString($bytes, $trailerAt, 32).ToLowerInvariant()
    if ((Get-Sha256Hex $bytes $Start ($trailerAt - $Start)) -cne $expectedTrailer) { return $null }
    $offset = $dataStart
    $entries = foreach ($header in $headers) {
        [pscustomobject]@{ name = $header.name; bytes = [int64]$header.size; sha256 = $header.sha256; offset = $offset }
        $offset += [int]$header.size
    }
    [pscustomobject]@{ start = $Start; length = $trailerAt + 32 - $Start; trailerSha256 = $expectedTrailer; entries = @($entries) }
}

$found = [Collections.Generic.List[object]]::new()
$index = $text.IndexOf($magicText, [StringComparison]::Ordinal)
while ($index -ge 0) {
    $container = Read-Container $index
    if ($container) { $found.Add($container) }
    $index = $text.IndexOf($magicText, $index + 1, [StringComparison]::Ordinal)
}
if ($found.Count -ne 1) { throw "Expected exactly one valid payload container in $SetupPath; found $($found.Count)." }
$container = $found[0]

$names = @($container.entries | ForEach-Object name)
$sortedExpected = [string[]]$ExpectedNames.Clone()
[Array]::Sort($sortedExpected, [StringComparer]::Ordinal)
if (($names -join '|') -cne ($sortedExpected -join '|')) {
    throw "Payload entries differ from the expected set: $($names -join ', ')"
}
foreach ($entry in $container.entries) {
    $actual = Get-Sha256Hex $bytes ([int]$entry.offset) ([int]$entry.bytes)
    if ($actual -cne $entry.sha256) { throw "Payload entry $($entry.name) does not match its SHA-256." }
    if ($ExpectedRoot) {
        $file = Join-Path $ExpectedRoot $entry.name
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Expected payload file is missing: $file" }
        $expected = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($expected -cne $entry.sha256) { throw "Payload entry $($entry.name) differs from $file." }
    }
}
if ($ExpectedRoot) {
    $rootSorted = [string[]]@(Get-ChildItem -LiteralPath $ExpectedRoot -Force | ForEach-Object Name)
    [Array]::Sort($rootSorted, [StringComparer]::Ordinal)
    if (($rootSorted -join '|') -cne ($names -join '|')) { throw "The expected payload folder holds other files: $($rootSorted -join ', ')" }
}
if ($ExtractTo) {
    [IO.Directory]::CreateDirectory($ExtractTo) | Out-Null
    foreach ($entry in $container.entries) {
        $target = Join-Path $ExtractTo $entry.name
        $stream = [IO.File]::Open($target, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
        try { $stream.Write($bytes, [int]$entry.offset, [int]$entry.bytes) } finally { $stream.Dispose() }
    }
}

[pscustomobject]@{
    containerOffset = $container.start
    containerBytes = $container.length
    trailerSha256 = $container.trailerSha256
    files = @($container.entries | ForEach-Object { [ordered]@{ name = $_.name; bytes = $_.bytes; sha256 = $_.sha256 } })
}
