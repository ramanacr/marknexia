[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

$ErrorActionPreference = 'Stop'
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
$script = Join-Path $RepositoryRoot 'scripts\Stage-WebView2Loader.ps1'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('marknexia-loader-test-' + [Guid]::NewGuid().ToString('N'))
try {
    [IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
    $fixture = Join-Path $temporaryRoot 'loader.pefixture'
    $bytes = [byte[]]::new(256)
    $bytes[0] = 0x4d; $bytes[1] = 0x5a
    [BitConverter]::GetBytes([int]128).CopyTo($bytes, 0x3c)
    $bytes[128] = 0x50; $bytes[129] = 0x45
    [BitConverter]::GetBytes([uint16]0x8664).CopyTo($bytes, 132)
    [BitConverter]::GetBytes([uint16]2).CopyTo($bytes, 148)
    [BitConverter]::GetBytes([uint16]0x20b).CopyTo($bytes, 152)
    [IO.File]::WriteAllBytes($fixture, $bytes)
    $package = Join-Path $temporaryRoot 'package.zip'
    Compress-Archive -LiteralPath $fixture -DestinationPath $package
    $lock = Join-Path $temporaryRoot 'loader.lock.json'
    $output = Join-Path $temporaryRoot 'staged.pefixture'
    @{
        schemaVersion='webview2-loader-lock-v1'; packageSha256=(Get-FileHash -LiteralPath $package -Algorithm SHA256).Hash.ToLowerInvariant()
        loaders=@{ x64=@{ entry='loader.pefixture'; bytes=256; sha256=(Get-FileHash -LiteralPath $fixture -Algorithm SHA256).Hash.ToLowerInvariant() } }
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $lock
    & $script -Architecture x64 -PackagePath $package -LockPath $lock -OutputPath $output | Out-Null
    Assert-That ((Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash -eq (Get-FileHash -LiteralPath $fixture -Algorithm SHA256).Hash) 'Staged loader bytes must match the pinned entry.'
    $tampered = Join-Path $temporaryRoot 'tampered.zip'
    [IO.File]::WriteAllBytes($tampered, [byte[]](1..10))
    $rejected = $false
    try { & $script -Architecture x64 -PackagePath $tampered -LockPath $lock -OutputPath $output | Out-Null }
    catch { $rejected = $_.Exception.Message -like '*package SHA-256*' }
    Assert-That $rejected 'Tampered package must fail before extraction.'
    Write-Host 'Pinned WebView2 loader staging regression tests passed.'
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) { Remove-Item -LiteralPath $temporaryRoot -Recurse -Force }
}
