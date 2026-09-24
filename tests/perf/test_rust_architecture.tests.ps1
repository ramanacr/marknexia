[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

$ErrorActionPreference = 'Stop'
$script = Join-Path $RepositoryRoot 'scripts\Test-RustArchitecture.ps1'
if (-not (Test-Path -LiteralPath $script -PathType Leaf)) {
    throw 'Test-RustArchitecture.ps1 must exist.'
}

function New-PeBytes([uint16]$Machine) {
    $bytes = [byte[]]::new(256)
    $bytes[0] = 0x4d
    $bytes[1] = 0x5a
    [BitConverter]::GetBytes([int]128).CopyTo($bytes, 0x3c)
    $bytes[128] = 0x50
    $bytes[129] = 0x45
    [BitConverter]::GetBytes($Machine).CopyTo($bytes, 132)
    [BitConverter]::GetBytes([uint16]2).CopyTo($bytes, 148)
    [BitConverter]::GetBytes([uint16]0x20b).CopyTo($bytes, 152)
    return $bytes
}

$x64Bytes = New-PeBytes 0x8664
$arm64Bytes = New-PeBytes 0xaa64
& $script -InputBytes $x64Bytes -Expected x64 | Out-Null
& $script -InputBytes $arm64Bytes -Expected ARM64 | Out-Null

foreach ($case in @(
    @{ bytes = $x64Bytes; expected = 'ARM64' },
    @{ bytes = $arm64Bytes; expected = 'x64' },
    @{ bytes = [byte[]](1..8); expected = 'x64' }
)) {
    $rejected = $false
    try {
        & $script -InputBytes $case.bytes -Expected $case.expected | Out-Null
    }
    catch {
        $rejected = $true
    }
    if (-not $rejected) {
        throw "Architecture check accepted invalid in-memory bytes as $($case.expected)."
    }
}

Write-Host 'Rust architecture in-memory regression tests passed.'
