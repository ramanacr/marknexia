[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
)

$ErrorActionPreference = 'Stop'
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) "Marknexia Build-Rust $([Guid]::NewGuid())"

function Write-BatchFile {
    param(
        [string]$Path,
        [string[]]$Lines
    )

    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $Path)) | Out-Null
    [System.IO.File]::WriteAllLines($Path, $Lines, [System.Text.Encoding]::ASCII)
}

try {
    [System.IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    $installRoot = Join-Path $temporaryRoot 'Visual Studio Fixture'
    $vsDevCmd = Join-Path $installRoot 'Common7\Tools\VsDevCmd.bat'
    $vsWhere = Join-Path $temporaryRoot 'tool chain\vswhere fixture.cmd'
    $cargo = Join-Path $temporaryRoot 'tool chain\cargo fixture.cmd'
    $vsRecord = Join-Path $temporaryRoot 'VsDevCmd arguments.txt'
    $cargoRecord = Join-Path $temporaryRoot 'Cargo arguments.txt'
    $buildScript = Join-Path $RepositoryRoot 'scripts\Build-Rust.ps1'
    $powerShell = Join-Path $PSHOME 'pwsh.exe'

    Write-BatchFile $vsWhere @('@echo off', "echo $installRoot", 'exit /b 0')
    Write-BatchFile $vsDevCmd @('@echo off', "echo %* >> `"$vsRecord`"", 'exit /b 0')
    Write-BatchFile $cargo @('@echo off', "echo %* >> `"$cargoRecord`"", 'exit /b %MARKNEXIA_TEST_CARGO_EXIT%')

    function Invoke-FakeBuild {
        param(
            [ValidateSet('x64', 'ARM64')]
            [string]$Architecture,
            [int]$CargoExitCode
        )

        $env:MARKNEXIA_TEST_CARGO_EXIT = $CargoExitCode
        & $powerShell -NoProfile -File $buildScript -Configuration Release -Architecture $Architecture `
            -RepositoryRoot $RepositoryRoot -CargoPath $cargo -VsWherePath $vsWhere -CmdPath $env:ComSpec
        return $LASTEXITCODE
    }

    if ((Invoke-FakeBuild -Architecture x64 -CargoExitCode 0) -ne 0) {
        throw 'Fake x64 build was expected to succeed.'
    }
    if ((Get-Content -LiteralPath $vsRecord -Raw) -notmatch '-arch=x64') {
        throw 'Fake x64 build did not select the x64 developer environment.'
    }
    if ((Get-Content -LiteralPath $cargoRecord -Raw) -notmatch 'x86_64-pc-windows-msvc') {
        throw 'Fake x64 build did not select the x64 Cargo target.'
    }

    if ((Invoke-FakeBuild -Architecture ARM64 -CargoExitCode 0) -ne 0) {
        throw 'Fake ARM64 build was expected to succeed.'
    }
    if ((Get-Content -LiteralPath $vsRecord -Raw) -notmatch '-arch=arm64') {
        throw 'Fake ARM64 build did not select the ARM64 developer environment.'
    }
    if ((Get-Content -LiteralPath $cargoRecord -Raw) -notmatch 'aarch64-pc-windows-msvc') {
        throw 'Fake ARM64 build did not select the ARM64 Cargo target.'
    }

    if ((Invoke-FakeBuild -Architecture ARM64 -CargoExitCode 17) -ne 17) {
        throw 'Build-Rust.ps1 did not propagate the child Cargo exit code.'
    }

    Write-Host 'Build-Rust behavioral contract passed.'
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
