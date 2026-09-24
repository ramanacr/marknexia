[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

$ErrorActionPreference = 'Stop'
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }

$script = Join-Path $RepositoryRoot 'scripts\Stage-WebView2Loader.ps1'
$source = Get-Content -LiteralPath $script -Raw
$guardText = "OutputPath must not overwrite PackagePath or LockPath."
$guardIndex = $source.IndexOf($guardText, [StringComparison]::Ordinal)
$firstWriteIndex = @(
    $source.IndexOf('[IO.Directory]::CreateDirectory', [StringComparison]::Ordinal),
    $source.IndexOf('[IO.File]::WriteAllBytes', [StringComparison]::Ordinal),
    $source.IndexOf('Move-Item', [StringComparison]::Ordinal)
) | Where-Object { $_ -ge 0 } | Sort-Object | Select-Object -First 1

Assert-That ($source.Contains('[IO.Path]::GetFullPath($OutputPath)') -and
    $source.Contains('[IO.Path]::GetFullPath($PackagePath)') -and
    $source.Contains('[IO.Path]::GetFullPath($LockPath)')) 'Output, package, and lock paths must be normalized before comparison.'
Assert-That ($source.Contains('[StringComparer]::OrdinalIgnoreCase')) 'Normalized Windows paths must be compared case-insensitively.'
Assert-That ($guardIndex -ge 0 -and $firstWriteIndex -ge 0 -and $guardIndex -lt $firstWriteIndex) 'Package/lock path collisions must be rejected before any output write.'
Assert-That ($source.Contains('$pathComparer.Equals($resolvedOutput, $resolvedPackage)') -and
    $source.Contains('$pathComparer.Equals($resolvedOutput, $resolvedLock)')) 'Both PackagePath and LockPath collisions must be rejected.'

Write-Host 'WebView2 loader source contracts passed; no package or loader fixture was written.'
