[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

$ErrorActionPreference = 'Stop'
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
$script = Join-Path $RepositoryRoot 'scripts\Stage-WebView2Loader.ps1'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('marknexia-loader-contract-test-' + [Guid]::NewGuid().ToString('N'))
try {
    [IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    $fixture = Join-Path $temporaryRoot 'not-a-loader.txt'
    'benign non-executable test content' | Set-Content -LiteralPath $fixture -NoNewline
    $package = Join-Path $temporaryRoot 'package.zip'
    Compress-Archive -LiteralPath $fixture -DestinationPath $package
    $lock = Join-Path $temporaryRoot 'loader.lock.json'
    $output = Join-Path $temporaryRoot 'staged.bin'
    @{
        schemaVersion='webview2-loader-lock-v1'; packageSha256=(Get-FileHash -LiteralPath $package -Algorithm SHA256).Hash.ToLowerInvariant()
        loaders=@{ x64=@{ entry='not-a-loader.txt'; bytes=(Get-Item -LiteralPath $fixture).Length; sha256=(Get-FileHash -LiteralPath $fixture -Algorithm SHA256).Hash.ToLowerInvariant() } }
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $lock
    $invalidLoaderRejected = $false
    try { & $script -Architecture x64 -PackagePath $package -LockPath $lock -OutputPath $output | Out-Null }
    catch { $invalidLoaderRejected = $_.Exception.Message -like '*valid PE image*' }
    Assert-That $invalidLoaderRejected 'Non-PE package content must be rejected in memory before staging.'
    Assert-That (-not (Test-Path -LiteralPath $output)) 'Rejected package content must never be written as a loader.'
    $tampered = Join-Path $temporaryRoot 'tampered.zip'
    [IO.File]::WriteAllBytes($tampered, [byte[]](1..10))
    $rejected = $false
    try { & $script -Architecture x64 -PackagePath $tampered -LockPath $lock -OutputPath $output | Out-Null }
    catch { $rejected = $_.Exception.Message -like '*package SHA-256*' }
    Assert-That $rejected 'Tampered package must fail before extraction.'
    Write-Host 'Pinned WebView2 loader rejection regression tests passed without writing PE bytes.'
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) { Remove-Item -LiteralPath $temporaryRoot -Recurse -Force }
}
