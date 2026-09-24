#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory, ParameterSetName = 'Path')][string]$Path,
    [Parameter(Mandatory, ParameterSetName = 'Bytes', DontShow)][byte[]]$InputBytes,
    [Parameter(Mandatory)][ValidateSet('x64', 'ARM64')][string]$Expected
)

$ErrorActionPreference = 'Stop'
$resolvedPath = '<in-memory>'
if ($PSCmdlet.ParameterSetName -eq 'Path') {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Executable was not found: $Path"
    }
    $resolvedPath = [IO.Path]::GetFullPath($Path)
    $stream = [IO.File]::OpenRead($resolvedPath)
}
else {
    $stream = [IO.MemoryStream]::new($InputBytes, $false)
}

$reader = [IO.BinaryReader]::new($stream)
try {
    $length = $reader.BaseStream.Length
    if ($length -lt 90 -or $reader.ReadUInt16() -ne 0x5a4d) {
        throw "Not a valid PE image (DOS header): $resolvedPath"
    }

    $reader.BaseStream.Position = 0x3c
    $peOffset = $reader.ReadInt32()
    if ($peOffset -lt 64 -or [int64]$peOffset + 26 -gt $length) {
        throw "Not a valid PE image (header offset): $resolvedPath"
    }

    $reader.BaseStream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x00004550) {
        throw "Not a valid PE image (signature): $resolvedPath"
    }
    $machine = $reader.ReadUInt16()
    $reader.BaseStream.Position = [int64]$peOffset + 20
    $optionalHeaderBytes = $reader.ReadUInt16()
    if ($optionalHeaderBytes -lt 2 -or [int64]$peOffset + 24 + $optionalHeaderBytes -gt $length) {
        throw "Not a valid PE image (optional header): $resolvedPath"
    }
    $reader.BaseStream.Position = [int64]$peOffset + 24
    if ($reader.ReadUInt16() -ne 0x020b) {
        throw "Not a 64-bit PE32+ image: $resolvedPath"
    }

    $expectedMachine = if ($Expected -eq 'x64') { 0x8664 } else { 0xaa64 }
    if ($machine -ne $expectedMachine) {
        throw ("PE machine mismatch for {0}: expected {1} (0x{2:x4}), found 0x{3:x4}." -f $resolvedPath, $Expected, $expectedMachine, $machine)
    }

    [pscustomobject]@{
        path = $resolvedPath
        architecture = $Expected
        machine = ('0x{0:x4}' -f $machine)
        evidenceType = 'static-pe-header-only'
        nativeRuntimeVerified = $false
    }
}
finally {
    $reader.Dispose()
}
