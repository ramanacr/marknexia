[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

$ErrorActionPreference = 'Stop'
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
$script = Join-Path $RepositoryRoot 'scripts\Compare-RustBuilds.ps1'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('marknexia-repro-test-' + [Guid]::NewGuid().ToString('N'))
try {
    $first = Join-Path $temporaryRoot 'first'
    $second = Join-Path $temporaryRoot 'second'
    [IO.Directory]::CreateDirectory($first) | Out-Null
    [IO.Directory]::CreateDirectory($second) | Out-Null
    $output = Join-Path $temporaryRoot 'comparison.json'
    [IO.File]::WriteAllBytes((Join-Path $first 'native.bin'), [byte[]](1..20))
    [IO.File]::WriteAllBytes((Join-Path $second 'native.bin'), [byte[]](1..20))
    & $script -FirstRoot $first -SecondRoot $second -OutputPath $output -RequiredRelativePaths @('native.bin') | Out-Null
    $result = Get-Content -LiteralPath $output -Raw | ConvertFrom-Json -AsHashtable
    Assert-That ($result.reproducible -eq $true -and $result.files.Count -eq 1) 'Matching clean builds must produce matching hash evidence.'
    [IO.File]::WriteAllBytes((Join-Path $second 'native.bin'), [byte[]](1..21))
    $rejected = $false
    try { & $script -FirstRoot $first -SecondRoot $second -OutputPath $output -RequiredRelativePaths @('native.bin') | Out-Null }
    catch { $rejected = $_.Exception.Message -like '*differ*' }
    Assert-That $rejected 'Mismatched build hashes must fail the reproducibility gate.'
    Write-Host 'Rust reproducible build comparison regression tests passed.'
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) { Remove-Item -LiteralPath $temporaryRoot -Recurse -Force }
}
