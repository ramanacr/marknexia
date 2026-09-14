[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release',
    [ValidateSet('x64', 'ARM64', 'All')]
    [string]$Architecture = 'All',
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
)

$ErrorActionPreference = 'Stop'
$cargoHome = if ($env:CARGO_HOME) {
    $env:CARGO_HOME
}
else {
    Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile)) '.cargo'
}
$cargoCandidates = @(
    (Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue),
    (Join-Path $cargoHome 'bin\cargo.exe')
) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }
$cargoPath = $cargoCandidates | Select-Object -First 1
if (-not $cargoPath) {
    throw 'Cargo was not found on PATH or under CARGO_HOME.'
}
$cmd = Get-Command cmd.exe -CommandType Application -ErrorAction Stop

function Resolve-VsDevCmd {
    $vswhereCandidates = @(
        (Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFilesX86)) 'Microsoft Visual Studio\Installer\vswhere.exe'),
        (Get-Command vswhere.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue)
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }

    $vswhere = $vswhereCandidates | Select-Object -First 1
    if (-not $vswhere) {
        throw 'Visual Studio discovery failed: vswhere.exe was not found.'
    }

    $installationPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $installationPath) {
        throw 'Visual Studio discovery failed: no C++ build tools installation was found.'
    }

    $vsDevCmd = Join-Path $installationPath.Trim() 'Common7\Tools\VsDevCmd.bat'
    if (-not (Test-Path -LiteralPath $vsDevCmd -PathType Leaf)) {
        throw "Visual Studio discovery failed: VsDevCmd.bat was not found at '$vsDevCmd'."
    }

    return $vsDevCmd
}

$vsDevCmd = Resolve-VsDevCmd
$targets = switch ($Architecture) {
    'x64' { @(@{ Triple = 'x86_64-pc-windows-msvc'; DeveloperArchitecture = 'x64' }) }
    'ARM64' { @(@{ Triple = 'aarch64-pc-windows-msvc'; DeveloperArchitecture = 'arm64' }) }
    default {
        @(
            @{ Triple = 'x86_64-pc-windows-msvc'; DeveloperArchitecture = 'x64' },
            @{ Triple = 'aarch64-pc-windows-msvc'; DeveloperArchitecture = 'arm64' }
        )
    }
}

foreach ($target in $targets) {
    $arguments = @('build', '--workspace', '--locked', '--target', $target.Triple)
    if ($Configuration -eq 'Release') {
        $arguments += '--release'
    }

    $cargoArguments = $arguments | ForEach-Object { '"{0}"' -f $_ }
    $commandLine = 'call "{0}" -no_logo -arch={1} -host_arch=x64 && "{2}" {3}' -f `
        $vsDevCmd, $target.DeveloperArchitecture, $cargoPath, ($cargoArguments -join ' ')

    Push-Location $RepositoryRoot
    try {
        & $cmd.Source /d /c $commandLine
    }
    finally {
        Pop-Location
    }
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}
