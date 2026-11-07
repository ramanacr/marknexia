#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('x64', 'ARM64')][string]$Architecture,
    [Parameter(Mandatory)][string]$PackagePath,
    [Parameter(Mandatory)][string]$OutputPath,
    [string]$LockPath = (Join-Path $PSScriptRoot '..\packaging\webview\loader.lock.json')
)

$ErrorActionPreference = 'Stop'
$resolvedOutput = [IO.Path]::GetFullPath($OutputPath)
$resolvedPackage = [IO.Path]::GetFullPath($PackagePath)
$resolvedLock = [IO.Path]::GetFullPath($LockPath)
$pathComparer = [StringComparer]::OrdinalIgnoreCase
if ($pathComparer.Equals($resolvedOutput, $resolvedPackage) -or $pathComparer.Equals($resolvedOutput, $resolvedLock)) {
    throw 'OutputPath must not overwrite PackagePath or LockPath.'
}
if (-not (Test-Path -LiteralPath $PackagePath -PathType Leaf)) { throw "WebView2 SDK package not found: $PackagePath" }
$lock = Get-Content -LiteralPath $LockPath -Raw | ConvertFrom-Json -AsHashtable
if ($lock.schemaVersion -ne 'webview2-loader-lock-v1' -or -not $lock.loaders.ContainsKey($Architecture)) {
    throw 'WebView2 loader lock does not cover the requested architecture.'
}
$actualPackageHash = (Get-FileHash -LiteralPath $PackagePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualPackageHash -ne $lock.packageSha256) { throw 'WebView2 package SHA-256 does not match the pinned Microsoft SDK package.' }

$expected = $lock.loaders[$Architecture]
$archive = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($PackagePath))
try {
    $entries = @($archive.Entries | Where-Object { $_.FullName -ceq $expected.entry })
    if ($entries.Count -ne 1) { throw 'Pinned WebView2 loader entry is missing or duplicated.' }
    $entry = $entries[0]
    if ($entry.Length -ne [int64]$expected.bytes -or $entry.Length -gt 1MB) { throw 'WebView2 loader entry length does not match the lock.' }
    $memory = [IO.MemoryStream]::new()
    try {
        $stream = $entry.Open()
        try { $stream.CopyTo($memory) } finally { $stream.Dispose() }
        $bytes = $memory.ToArray()
    }
    finally { $memory.Dispose() }
}
finally { $archive.Dispose() }

$actualEntryHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
if ($actualEntryHash -ne $expected.sha256) { throw 'WebView2 loader entry SHA-256 does not match the lock.' }
& (Join-Path $PSScriptRoot 'Test-RustArchitecture.ps1') -InputBytes $bytes -Expected $Architecture | Out-Null
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($resolvedOutput)) | Out-Null
$temporaryOutput = $resolvedOutput + '.staging.bin'
try {
    [IO.File]::WriteAllBytes($temporaryOutput, $bytes)
    Move-Item -LiteralPath $temporaryOutput -Destination $resolvedOutput -Force
}
finally {
    if (Test-Path -LiteralPath $temporaryOutput -PathType Leaf) { Remove-Item -LiteralPath $temporaryOutput -Force }
}
Write-Host "Pinned WebView2 loader staged for ${Architecture}: $resolvedOutput"
