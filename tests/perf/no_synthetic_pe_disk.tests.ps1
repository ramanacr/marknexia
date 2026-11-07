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

# Executable file names. The development machines' antivirus watches for new files named
# *.exe or *.dll, even with text content, so local tests use non-executable stand-in names
# (for example shell.bin.txt). Real names may appear only in memory or in CI-only steps.
# Rust test code (crates/**/tests and everything after #[cfg(test)]) must not join an
# executable name, literal or through a constant bound to one, into a path in a file that
# writes to disk, and no line may write to an executable-named literal path.
$exeLiteral = '(?i)r?#*"[^"\r\n]*\.(exe|dll)"#*'
$rustWrite = '\b(fs::write|File::create|create_new|OpenOptions|fs::copy|fs::rename|write_all|create_dir_all)\b'
function Get-ExecutableConstants([string[]]$Texts) {
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($text in $Texts) {
        foreach ($match in [regex]::Matches($text, "const\s+([A-Za-z0-9_]+)\s*:\s*&(?:'static\s+)?str\s*=\s*$exeLiteral")) {
            [void]$names.Add($match.Groups[1].Value)
        }
    }
    , $names
}
function Get-RustTestCode([string]$Path, [string]$Text) {
    if ($Path -match '[\\/]tests[\\/]') { return $Text }
    $start = $Text.IndexOf('#[cfg(test)]', [StringComparison]::Ordinal)
    if ($start -lt 0) { return '' }
    $Text.Substring($start)
}
function Find-ExecutableWrites([string]$TestCode, [Collections.Generic.HashSet[string]]$Constants) {
    $found = [Collections.Generic.List[string]]::new()
    if ($TestCode -notmatch $rustWrite) { return }
    $names = @($Constants | ForEach-Object { [regex]::Escape($_) })
    $joined = if ($names.Count -gt 0) { "\.join\(\s*(?:&\s*)?(?:[A-Za-z_]+::)*(?:$($names -join '|'))\b" } else { $null }
    foreach ($match in [regex]::Matches($TestCode, "\.join\(\s*$exeLiteral")) { $found.Add($match.Value) }
    if ($joined) { foreach ($match in [regex]::Matches($TestCode, $joined)) { $found.Add($match.Value) } }
    foreach ($line in $TestCode -split "`n") {
        if ($line -match $rustWrite -and $line -match $exeLiteral) { $found.Add($line.Trim()) }
    }
    $found.ToArray()
}

# Self-check of the detector on in-memory samples.
$sampleConstants = Get-ExecutableConstants @('pub const SHELL: &str = "marknexia-win32.exe";')
if (@(Find-ExecutableWrites "#[cfg(test)] fn t() { std::fs::write(dir.join(SHELL), b`"x`"); }" $sampleConstants).Count -eq 0 -or
    @(Find-ExecutableWrites "#[cfg(test)] fn t() { std::fs::write(dir.join(`"a.dll`"), b`"x`"); }" $sampleConstants).Count -eq 0 -or
    @(Find-ExecutableWrites "#[cfg(test)] fn t() { std::fs::write(dir.join(`"shell.bin.txt`"), b`"x`"); }" $sampleConstants).Count -ne 0 -or
    @(Find-ExecutableWrites "#[cfg(test)] fn t() { let p = dir.join(SHELL); assert!(p.ends_with(`"x`")); }" $sampleConstants).Count -ne 0) {
    $violations.Add('executable-name detector self-check failed')
}

$rustFiles = @(Get-ChildItem -LiteralPath (Join-Path $RepositoryRoot 'crates') -Recurse -File -Filter '*.rs')
$rustTexts = @{}
foreach ($file in $rustFiles) { $rustTexts[$file.FullName] = [IO.File]::ReadAllText($file.FullName) }
$executableConstants = Get-ExecutableConstants @($rustTexts.Values)
# Files that are test-only because a #[cfg(test)] module declaration includes them.
$testOnlyFiles = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($file in $rustFiles) {
    foreach ($match in [regex]::Matches($rustTexts[$file.FullName], '#\[cfg\(test\)\]\s*(?:pub(?:\([a-z]+\))?\s+)?mod\s+([A-Za-z0-9_]+)\s*;')) {
        foreach ($candidate in @("$($match.Groups[1].Value).rs", "$($match.Groups[1].Value)\mod.rs")) {
            [void]$testOnlyFiles.Add([IO.Path]::GetFullPath((Join-Path $file.DirectoryName $candidate)))
        }
    }
}
foreach ($file in $rustFiles) {
    $testCode = if ($testOnlyFiles.Contains($file.FullName)) { $rustTexts[$file.FullName] } else { Get-RustTestCode $file.FullName $rustTexts[$file.FullName] }
    if (-not $testCode) { continue }
    foreach ($hit in Find-ExecutableWrites $testCode $executableConstants) {
        $violations.Add("$([IO.Path]::GetRelativePath($RepositoryRoot, $file.FullName)): test code writes an executable-named path ($hit)")
    }
}
$psWrite = '\b(WriteAllBytes|WriteAllText|Set-Content|Out-File|OpenWrite|CreateNew|New-Item|Copy-Item|Move-Item)\b'
foreach ($file in Get-ChildItem -LiteralPath (Join-Path $RepositoryRoot 'tests') -Recurse -File -Filter '*.ps1') {
    if ([IO.Path]::GetFullPath($file.FullName) -eq $self) { continue }
    foreach ($line in [IO.File]::ReadAllLines($file.FullName)) {
        if ($line -match $psWrite -and $line -match "(?i)['`"][^'`"]*\.(exe|dll)['`"]") {
            $violations.Add("$($file.Name): writes an executable-named path ($($line.Trim()))")
        }
    }
}

if ($violations.Count -gt 0) {
    throw "Synthetic PE disk-safety contract failed:`n$($violations -join "`n")"
}

Write-Host 'Synthetic PE disk-safety source contract passed.'
