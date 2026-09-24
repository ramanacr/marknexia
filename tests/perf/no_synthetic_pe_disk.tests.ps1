[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

$ErrorActionPreference = 'Stop'
$self = [IO.Path]::GetFullPath($PSCommandPath)
$violations = [Collections.Generic.List[string]]::new()

foreach ($file in Get-ChildItem -LiteralPath (Join-Path $RepositoryRoot 'tests\perf') -File -Filter '*.tests.ps1') {
    if ([IO.Path]::GetFullPath($file.FullName) -eq $self) { continue }
    $text = Get-Content -LiteralPath $file.FullName -Raw
    if ($text.Contains('.pefixture') -or $text.Contains('New-PeFixture')) {
        $violations.Add("$($file.Name): legacy disk PE fixture marker")
    }
    $writesBytes = $text -match '\b(WriteAllBytes|WriteAllText|Set-Content|Out-File|OpenWrite|CreateNew|FileStream)\b'
    $constructsPeHeader = $text -match '\[0\]\s*=\s*0x4d' -and
        $text -match '\[1\]\s*=\s*0x5a' -and
        (($text -match '\[128\]\s*=\s*0x50' -and $text -match '\[129\]\s*=\s*0x45') -or $text.Contains('0x00004550'))
    if ($writesBytes -and $constructsPeHeader) {
        $violations.Add("$($file.Name): constructs PE bytes and writes bytes to disk")
    }

    $createsArchive = $text -match '\bCompress-Archive\b' -or
        $text -match '\[IO\.Compression\.ZipFile\]::CreateFromDirectory' -or
        $text -match '\[IO\.Compression\.ZipArchive\]::new\([^\r\n]*ZipArchiveMode\.Create' -or
        $text -match '\.Open\([^\r\n]*ZipArchiveMode\.Create'
    $writesArchivePath = $text -match '(?i)["''][^"'']+\.(zip|nupkg|7z|tar|gz)["'']' -and $writesBytes
    if ($createsArchive -or $writesArchivePath) {
        $violations.Add("$($file.Name): creates or writes a synthetic archive on disk")
    }
}

$architectureScript = Get-Content -LiteralPath (Join-Path $RepositoryRoot 'scripts\Test-RustArchitecture.ps1') -Raw
if (-not $architectureScript.Contains("ParameterSetName = 'Bytes'")) {
    $violations.Add('Test-RustArchitecture.ps1: missing in-memory byte parameter set')
}

$loaderScript = Get-Content -LiteralPath (Join-Path $RepositoryRoot 'scripts\Stage-WebView2Loader.ps1') -Raw
$validationIndex = $loaderScript.IndexOf('-InputBytes $bytes', [StringComparison]::Ordinal)
$writeIndex = $loaderScript.IndexOf('WriteAllBytes', [StringComparison]::Ordinal)
if ($validationIndex -lt 0 -or $writeIndex -lt 0 -or $validationIndex -gt $writeIndex) {
    $violations.Add('Stage-WebView2Loader.ps1: loader bytes are not validated in memory before disk write')
}

if ($violations.Count -gt 0) {
    throw "Synthetic PE disk-safety contract failed:`n$($violations -join "`n")"
}

Write-Host 'Synthetic PE disk-safety source contract passed.'
