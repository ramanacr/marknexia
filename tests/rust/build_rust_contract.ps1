[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
)

$ErrorActionPreference = 'Stop'
$scriptPath = Join-Path $RepositoryRoot 'scripts\Build-Rust.ps1'
$script = Get-Content -LiteralPath $scriptPath -Raw

$requiredPatterns = @(
    'vswhere\.exe',
    'VsDevCmd\.bat',
    'cmd\.exe',
    '/d /c',
    'CARGO_HOME',
    'cargo\.exe',
    '--locked',
    'LASTEXITCODE'
)

foreach ($pattern in $requiredPatterns) {
    if ($script -notmatch $pattern) {
        throw "Build-Rust.ps1 must contain '$pattern' to preserve the portable MSVC build contract."
    }
}

Write-Host 'Build-Rust developer-environment contract passed.'
