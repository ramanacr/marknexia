[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

$ErrorActionPreference = 'Stop'
$script = Join-Path $RepositoryRoot 'scripts\Test-RustArchitecture.ps1'
if (-not (Test-Path -LiteralPath $script -PathType Leaf)) {
    throw 'Test-RustArchitecture.ps1 must exist.'
}

$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('marknexia-pe-test-' + [Guid]::NewGuid().ToString('N'))
$resolvedTemp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$resolvedTarget = [IO.Path]::GetFullPath($temporaryRoot)
if (-not $resolvedTarget.StartsWith($resolvedTemp, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Test directory escaped the system temporary directory.'
}

function New-PeFixture([string]$Path, [uint16]$Machine) {
    $bytes = [byte[]]::new(256)
    $bytes[0] = 0x4d
    $bytes[1] = 0x5a
    [BitConverter]::GetBytes([int]128).CopyTo($bytes, 0x3c)
    $bytes[128] = 0x50
    $bytes[129] = 0x45
    [BitConverter]::GetBytes($Machine).CopyTo($bytes, 132)
    [BitConverter]::GetBytes([uint16]2).CopyTo($bytes, 148)
    [BitConverter]::GetBytes([uint16]0x20b).CopyTo($bytes, 152)
    [IO.File]::WriteAllBytes($Path, $bytes)
}

try {
    [IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    $x64Path = Join-Path $temporaryRoot 'x64.pefixture'
    $arm64Path = Join-Path $temporaryRoot 'arm64.pefixture'
    $invalidPath = Join-Path $temporaryRoot 'invalid.pefixture'
    # Synthetic PE bytes are parsed as data only; never give them an executable filename.
    foreach ($fixturePath in @($x64Path, $arm64Path, $invalidPath)) {
        if ([IO.Path]::GetExtension($fixturePath) -in @('.exe', '.dll')) {
            throw "Synthetic PE fixture must not use an executable filename: $fixturePath"
        }
    }
    New-PeFixture $x64Path 0x8664
    New-PeFixture $arm64Path 0xaa64
    [IO.File]::WriteAllBytes($invalidPath, [byte[]](1..8))

    & $script -Path $x64Path -Expected x64 | Out-Null
    & $script -Path $arm64Path -Expected ARM64 | Out-Null

    foreach ($case in @(
        @{ path = $x64Path; expected = 'ARM64' },
        @{ path = $arm64Path; expected = 'x64' },
        @{ path = $invalidPath; expected = 'x64' }
    )) {
        $rejected = $false
        try {
            & $script -Path $case.path -Expected $case.expected | Out-Null
        }
        catch {
            $rejected = $true
        }
        if (-not $rejected) {
            throw "Architecture check accepted an invalid fixture: $($case.path) as $($case.expected)."
        }
    }
    Write-Host 'Rust architecture static-check regression tests passed.'
}
finally {
    if (Test-Path -LiteralPath $resolvedTarget -PathType Container) {
        Remove-Item -LiteralPath $resolvedTarget -Recurse -Force
    }
}
