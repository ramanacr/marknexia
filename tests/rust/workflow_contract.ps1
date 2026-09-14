[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
)

$ErrorActionPreference = 'Stop'
$workflowPath = Join-Path $RepositoryRoot '.github\workflows\rust-feasibility.yml'
$workflow = Get-Content -LiteralPath $workflowPath -Raw

if ($workflow -notmatch 'tests/rust/portable_dependency_boundary\.ps1') {
    throw 'Rust feasibility workflow must run the portable dependency boundary gate.'
}

Write-Host 'Rust feasibility workflow contract passed.'
