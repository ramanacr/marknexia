[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

# Headless contract test for scripts/Test-RustSetupPayload.ps1. The container is built here
# from text entries only (no executable bytes) and wrapped in text, as the linker places it
# inside the setup executable. Nothing is run.

$ErrorActionPreference = 'Stop'
$script = Join-Path $RepositoryRoot 'scripts\Test-RustSetupPayload.ps1'
$utf8 = [Text.UTF8Encoding]::new($false)
$work = Join-Path ([IO.Path]::GetTempPath()) "marknexia-setup-payload-test-$([Guid]::NewGuid().ToString('N'))"
[IO.Directory]::CreateDirectory($work) | Out-Null

function New-Container([object[]]$Files) {
    $sha = [Security.Cryptography.SHA256]::Create()
    $stream = [IO.MemoryStream]::new()
    $writer = [IO.BinaryWriter]::new($stream)
    $writer.Write([Text.Encoding]::ASCII.GetBytes('MNXSETUP-PAYLD-1'))
    $writer.Write([uint32]$Files.Count)
    foreach ($file in $Files) {
        $name = [Text.Encoding]::ASCII.GetBytes($file.Name)
        $writer.Write([uint16]$name.Length); $writer.Write($name)
        $writer.Write([uint64]$file.Bytes.Length); $writer.Write($sha.ComputeHash($file.Bytes))
    }
    foreach ($file in $Files) { $writer.Write($file.Bytes) }
    $writer.Flush()
    $body = $stream.ToArray()
    , ([byte[]]($body + $sha.ComputeHash($body)))
}

try {
    $names = @('a.txt', 'b.txt', 'c.txt')
    $root = Join-Path $work 'expected'
    [IO.Directory]::CreateDirectory($root) | Out-Null
    $files = foreach ($name in $names) {
        $data = $utf8.GetBytes("text payload $name`n")
        [IO.File]::WriteAllBytes((Join-Path $root $name), $data)
        [pscustomobject]@{ Name = $name; Bytes = $data }
    }
    $container = New-Container $files
    # A decoy magic (as the parser's own constant appears in code) before the real container.
    $prefix = $utf8.GetBytes('code ... MNXSETUP-PAYLD-1 not a container ... ')
    $suffix = $utf8.GetBytes(' ... more sections')
    $image = Join-Path $work 'setup-stand-in.bin'
    [IO.File]::WriteAllBytes($image, [byte[]]($prefix + $container + $suffix))

    $result = & $script -SetupPath $image -ExpectedRoot $root -ExpectedNames $names
    if ($result.files.Count -ne 3 -or $result.containerOffset -ne $prefix.Length) { throw 'The container was not found at its offset.' }
    if ($result.files[1].sha256 -ne (Get-FileHash -LiteralPath (Join-Path $root 'b.txt') -Algorithm SHA256).Hash.ToLowerInvariant()) {
        throw 'Entry hash not reported.'
    }

    $failures = [Collections.Generic.List[string]]::new()
    function Assert-Rejected([string]$Label, [scriptblock]$Action) {
        try { & $Action | Out-Null; $failures.Add("$Label was accepted") } catch { }
    }
    # A changed data byte breaks the trailer.
    $damaged = [byte[]]($prefix + $container + $suffix)
    $damaged[$prefix.Length + $container.Length - 40] = $damaged[$prefix.Length + $container.Length - 40] -bxor 1
    $damagedPath = Join-Path $work 'damaged.bin'
    [IO.File]::WriteAllBytes($damagedPath, $damaged)
    Assert-Rejected 'a damaged container' { & $script -SetupPath $damagedPath -ExpectedNames $names }
    # A different file set.
    Assert-Rejected 'a different file set' { & $script -SetupPath $image -ExpectedNames @('a.txt', 'b.txt') }
    # An expected file that differs.
    [IO.File]::WriteAllBytes((Join-Path $root 'c.txt'), $utf8.GetBytes('changed'))
    Assert-Rejected 'a payload that differs from the expected folder' { & $script -SetupPath $image -ExpectedRoot $root -ExpectedNames $names }
    # Two valid containers.
    $twice = Join-Path $work 'twice.bin'
    [IO.File]::WriteAllBytes($twice, [byte[]]($container + $suffix + $container))
    Assert-Rejected 'two containers' { & $script -SetupPath $twice -ExpectedNames $names }
    # Extraction is CI only.
    if ($env:GITHUB_ACTIONS -ne 'true') {
        Assert-Rejected 'local extraction' { & $script -SetupPath $image -ExpectedNames $names -ExtractTo (Join-Path $work 'x') }
    }
    # Source contracts: the CI-only scripts refuse to run before they build, extract or run anything.
    $contracts = @(
        @{ Script = 'New-RustInstallerPackages.ps1'; Before = @('cargo build', '& $makeAppx', '-ExtractTo $extracted') },
        @{ Script = 'Test-RustSetupInstall.ps1'; Before = @('Start-Process', 'Invoke-Setup $SetupPath') }
    )
    foreach ($contract in $contracts) {
        $source = Get-Content -LiteralPath (Join-Path $RepositoryRoot "scripts\$($contract.Script)") -Raw
        $guard = $source.IndexOf("`$env:GITHUB_ACTIONS -ne 'true'", [StringComparison]::Ordinal)
        if ($guard -lt 0) { $failures.Add("$($contract.Script) has no GitHub Actions guard"); continue }
        foreach ($marker in $contract.Before) {
            $at = $source.IndexOf($marker, [StringComparison]::Ordinal)
            if ($at -ge 0 -and $at -lt $guard) { $failures.Add("$($contract.Script) uses '$marker' before its guard") }
        }
    }
    $installers = Get-Content -LiteralPath (Join-Path $RepositoryRoot 'scripts\New-RustInstallerPackages.ps1') -Raw
    foreach ($forbidden in @('signtool', 'New-SelfSignedCertificate', 'gh release', 'Publish-')) {
        if ($installers.Contains($forbidden)) { $failures.Add("New-RustInstallerPackages.ps1 must not sign or publish ('$forbidden')") }
    }
    if ($failures.Count -gt 0) { throw "Setup payload verifier contract failed:`n$($failures -join "`n")" }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host 'Setup payload verifier contract passed.'
