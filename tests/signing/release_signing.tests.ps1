# Headless tests for the local signing flow (scripts/signing/MarknexiaSigning.psm1,
# scripts/Invoke-ReleaseSigning.ps1, scripts/Test-SignedHandoff.ps1 and the workflows).
# signtool, gh and Get-AuthenticodeSignature are replaced by script blocks. Stand-in files
# are plain text with .bin, .zipx and .msixx names; PE bytes exist only in memory and are
# never written. No certificate, network or archive is used.
[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $RepositoryRoot 'scripts\signing\MarknexiaSigning.psm1') -Force
$failures = [Collections.Generic.List[string]]::new()
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { $failures.Add($Message) } }
function Assert-Throws([scriptblock]$Block, [string]$Pattern, [string]$Message) {
    try { & $Block; $failures.Add("$Message (no error)") }
    catch { if ($_.Exception.Message -notmatch $Pattern) { $failures.Add("$Message (wrong error: $($_.Exception.Message))") } }
}

$config = Get-ReleaseSigningConfig -Path (Join-Path $RepositoryRoot 'packaging\signing\release-signing.json')
$work = Join-Path ([IO.Path]::GetTempPath()) "marknexia-signing-tests-$([Guid]::NewGuid().ToString('N'))"
[IO.Directory]::CreateDirectory($work) | Out-Null

try {
    # --- Configuration ----------------------------------------------------------------------
    Assert-That ($config.thumbprint -ceq '68572404AE99971D76B907D9452001DCEE7766F1') 'Configured thumbprint changed.'
    Assert-That ($config.subject -ceq 'CN=Open Source Developer Ramana Reddy Chamakura, O=Open Source Developer, L=Hyderabad, S=Telangana, C=IN') 'Configured subject changed.'
    Assert-That ($config.timestampUrl -ceq 'http://time.certum.pl') 'Timestamp server must be Certum.'
    Assert-That ((Get-HandoffTag 'rust' '42') -ceq 'signing-handoff-rust-42') 'Handoff tag format.'
    Assert-Throws { Get-HandoffTag 'rust' '4x2' } 'numeric' 'Handoff tag needs a numeric run id.'
    Assert-That ((ConvertTo-HandoffAssetName 'arm64' 7 'runtimes/win/lib x.dll') -ceq 'arm64-007-lib_x.dll') 'Handoff asset names are flat and sanitised.'
    $rustSpec = Get-ProductSpec -Product Rust
    $dotnetSpec = Get-ProductSpec -Product DotNet
    Assert-That ('rust-preview-0.2.0' -cmatch $rustSpec.TagPattern -and 'v1.0.0' -cnotmatch $rustSpec.TagPattern) 'Rust tags must be rust-preview-* and never v*.'
    Assert-That ('v1.0.19' -cmatch $dotnetSpec.TagPattern -and 'rust-preview-1' -cnotmatch $dotnetSpec.TagPattern) '.NET tags must be v*.'
    Assert-That ($rustSpec.Prerelease -and -not $dotnetSpec.Prerelease) 'Rust stays a pre-release; .NET is a release.'
    Assert-That ((@($rustSpec.OwnerFiles) -join ',') -ceq 'marknexia-win32.exe') 'Rust signs only the shell; the WebView2 loader keeps its Microsoft signature.'

    # --- Signtool arguments -------------------------------------------------------------------
    $peArgs = Get-SignToolSignArguments -Config $config -Kind Pe -Paths @('C:\x\a.bin')
    Assert-That (($peArgs[0..8] -join ' ') -ceq "sign /fd SHA256 /tr http://time.certum.pl /td SHA256 /sha1 $($config.thumbprint)") "Signing arguments: $($peArgs -join ' ')"
    Assert-That ($peArgs -contains '/d' -and $peArgs[-1] -ceq 'C:\x\a.bin') 'PE signing names the description and ends with the file.'
    $msixArgs = Get-SignToolSignArguments -Config $config -Kind Msix -Paths @('C:\x\a.msixx')
    Assert-That ($msixArgs -notcontains '/d' -and $msixArgs -contains '/tr') 'MSIX signing is timestamped and has no PE description.'
    $probeArgs = Get-SignToolSignArguments -Config $config -Kind Probe -Paths @('C:\x\p.ps1')
    Assert-That ($probeArgs -notcontains '/tr') 'The session probe is not timestamped.'
    foreach ($forbidden in '/f', '/p', '/csp', '/kc', '/ac') {
        Assert-That ($peArgs -notcontains $forbidden -and $msixArgs -notcontains $forbidden) "Signing must not pass $forbidden (no key files or passwords)."
    }

    # --- PE content digest (in memory only) -----------------------------------------------------
    function New-TestImage([int]$Length, [int]$Magic) {
        $bytes = [byte[]]::new($Length)
        for ($i = 0x200; $i -lt $Length; $i++) { $bytes[$i] = [byte]($i % 251) }
        $bytes[0] = 0x4D; $bytes[1] = 0x5A
        [BitConverter]::GetBytes([int]0x80).CopyTo($bytes, 0x3C)
        [byte[]]@(0x50, 0x45, 0, 0) | ForEach-Object -Begin { $k = 0x80 } -Process { $bytes[$k++] = $_ }
        $optionalSize = if ($Magic -eq 0x20B) { 240 } else { 224 }
        [BitConverter]::GetBytes([uint16]$optionalSize).CopyTo($bytes, 0x80 + 20)
        [BitConverter]::GetBytes([uint16]$Magic).CopyTo($bytes, 0x98)
        $countOffset = if ($Magic -eq 0x20B) { 108 } else { 92 }
        [BitConverter]::GetBytes([uint32]16).CopyTo($bytes, 0x98 + $countOffset)
        , $bytes
    }
    function Add-TestSignature([byte[]]$Image, [int]$Magic, [int]$TableSize = 40) {
        $pad = (8 - ($Image.Length % 8)) % 8
        $signed = [byte[]]::new($Image.Length + $pad + $TableSize)
        [Array]::Copy($Image, $signed, $Image.Length)
        for ($i = $Image.Length + $pad; $i -lt $signed.Length; $i++) { $signed[$i] = 0xA5 }
        [BitConverter]::GetBytes([uint32]0x12345678).CopyTo($signed, 0x98 + 64)
        $entry = 0x98 + $(if ($Magic -eq 0x20B) { 112 } else { 96 }) + 32
        [BitConverter]::GetBytes([uint32]($Image.Length + $pad)).CopyTo($signed, $entry)
        [BitConverter]::GetBytes([uint32]$TableSize).CopyTo($signed, $entry + 4)
        , $signed
    }
    foreach ($case in @(@{ Length = 1024; Magic = 0x20B }, @{ Length = 1021; Magic = 0x20B }, @{ Length = 1536; Magic = 0x10B })) {
        $image = New-TestImage $case.Length $case.Magic
        $signed = Add-TestSignature $image $case.Magic
        $unsignedDigest = Get-PeContentDigest -Bytes $image
        Assert-That ($unsignedDigest -ceq (Get-PeContentDigest -Bytes $signed)) "Signing must not change the content digest ($($case.Length) bytes, magic $($case.Magic))."
        $changed = [byte[]]$signed.Clone(); $changed[0x300] = [byte]($changed[0x300] -bxor 1)
        Assert-That ($unsignedDigest -cne (Get-PeContentDigest -Bytes $changed)) 'A changed code byte must change the content digest.'
    }
    $image = New-TestImage 1024 0x20B
    $signed = Add-TestSignature $image 0x20B
    $badTable = [byte[]]$signed.Clone()
    [BitConverter]::GetBytes([uint32]0xFFFF).CopyTo($badTable, 0x98 + 112 + 32 + 4)
    Assert-Throws { Get-PeContentDigest -Bytes $badTable } 'outside the file' 'A certificate table past the end must be refused.'
    $notPe = [byte[]]::new(256)
    Assert-Throws { Get-PeContentDigest -Bytes $notPe } 'MZ' 'Non-PE bytes must be refused.'

    # --- Certificate checks ---------------------------------------------------------------------
    $raw = [Text.Encoding]::ASCII.GetBytes('stand-in certificate bytes')
    $fakeConfig = $config | ConvertTo-Json | ConvertFrom-Json
    $fakeConfig.certificateSha256 = ([Security.Cryptography.SHA256]::HashData($raw) | ForEach-Object { $_.ToString('x2') }) -join ''
    function New-FakeCertificate([hashtable]$Override = @{}) {
        $cert = [pscustomobject]@{
            Subject = $config.subject; RawData = $raw; NotBefore = [datetime]'2026-11-25'; NotAfter = [datetime]'2027-11-25'; HasPrivateKey = $true
            EnhancedKeyUsageList = @([pscustomobject]@{ ObjectId = '1.3.6.1.5.5.7.3.3' })
        }
        foreach ($key in $Override.Keys) { $cert.$key = $Override[$key] }
        $cert
    }
    $now = [datetime]'2027-01-01'
    try { Assert-SigningCertificate -Config $fakeConfig -Certificate (New-FakeCertificate) -Now $now } catch { $failures.Add("A valid certificate was refused: $_") }
    Assert-Throws { Assert-SigningCertificate -Config $fakeConfig -Certificate (New-FakeCertificate) -Now ([datetime]'2028-01-01') } 'expired' 'An expired certificate must be refused.'
    Assert-Throws { Assert-SigningCertificate -Config $fakeConfig -Certificate (New-FakeCertificate @{ HasPrivateKey = $false }) -Now $now } 'SimplySign' 'A certificate without a key must point to SimplySign.'
    Assert-Throws { Assert-SigningCertificate -Config $fakeConfig -Certificate (New-FakeCertificate @{ EnhancedKeyUsageList = @() }) -Now $now } 'code-signing' 'A certificate without code-signing usage must be refused.'
    Assert-Throws { Assert-SigningCertificate -Config $fakeConfig -Certificate (New-FakeCertificate @{ Subject = 'CN=Marknexia' }) -Now $now } 'subject' 'Another subject (for example the old self-signed CN=Marknexia) must be refused.'
    Assert-Throws { Assert-SigningCertificate -Config $config -Certificate (New-FakeCertificate) -Now $now } 'SHA-256' 'A certificate with another SHA-256 must be refused.'

    # --- Mock tools -------------------------------------------------------------------------------
    $script:signatures = @{}
    $script:signCalls = [Collections.Generic.List[object]]::new()
    $script:ghCalls = [Collections.Generic.List[string]]::new()
    $script:sleeps = [Collections.Generic.List[int]]::new()
    $script:log = [Collections.Generic.List[string]]::new()
    $script:signFailures = 0
    $script:runs = @{}; $script:artifacts = @{}; $script:releases = @{}; $script:tags = @{}
    $script:handoffManifest = ''; $script:archiveEntries = @{}; $script:msixPublisher = $config.subject
    function Write-StandIn([string]$Path, [string]$Content) {
        [IO.Directory]::CreateDirectory((Split-Path $Path -Parent)) | Out-Null
        [IO.File]::WriteAllText($Path, $Content)
    }
    function New-GhResult([int]$Exit = 0, [string]$Output = '', [string]$ErrorText = '') { [pscustomobject]@{ ExitCode = $Exit; Output = $Output; Error = $ErrorText } }
    $tools = [pscustomobject]@{
        SignTool           = {
            param([string[]]$Arguments)
            $script:signCalls.Add(($Arguments -join ' | '))
            if ($Arguments[0] -eq 'verify') {
                $known = $script:signatures.ContainsKey($Arguments[-1].ToLowerInvariant())
                return New-GhResult $(if ($known) { 0 } else { 1 }) 'verify'
            }
            if ($script:signFailures -gt 0) { $script:signFailures--; return New-GhResult 1 'SignTool Error: The specified timestamp server could not be reached.' }
            $thumb = $Arguments[[Array]::IndexOf($Arguments, '/sha1') + 1]
            foreach ($file in $Arguments | Where-Object { [IO.Path]::IsPathRooted($_) -and (Test-Path -LiteralPath $_) }) {
                $script:signatures[$file.ToLowerInvariant()] = @{ Thumb = $thumb; Timestamped = ($Arguments -contains '/tr') }
            }
            New-GhResult 0 'Successfully signed'
        }
        Gh                 = { param([string[]]$Arguments) $script:ghCalls.Add($Arguments -join ' '); & $script:ghHandler $Arguments }
        GetSignature       = {
            param([string]$Path)
            $key = $Path.ToLowerInvariant()
            if ($script:signatures.ContainsKey($key)) {
                $s = $script:signatures[$key]
                return [pscustomobject]@{ Status = 'Valid'; Thumbprint = $s.Thumb; Subject = 'CN=Owner'; Timestamped = $s.Timestamped; StatusMessage = '' }
            }
            $text = [IO.File]::ReadAllText($Path)
            if ($text.StartsWith('THIRDPARTY')) { return [pscustomobject]@{ Status = 'Valid'; Thumbprint = ('B' * 40); Subject = 'CN=Microsoft Corporation, O=Microsoft Corporation'; Timestamped = $true; StatusMessage = '' } }
            if ($text.StartsWith('BADSIG')) { return [pscustomobject]@{ Status = 'HashMismatch'; Thumbprint = ('C' * 40); Subject = 'CN=x'; Timestamped = $true; StatusMessage = 'hash mismatch' } }
            [pscustomobject]@{ Status = 'NotSigned'; Thumbprint = $null; Subject = $null; Timestamped = $false; StatusMessage = '' }
        }
        ContentDigest      = { param([string]$Path) Get-FileSha256 $Path }
        ArchiveEntrySha256 = { param([string]$ArchivePath, [string]$EntryName) $script:archiveEntries[(Split-Path $ArchivePath -Leaf)] }
        MsixPublisher      = { param([string]$Path) $script:msixPublisher }
        Sleep              = { param([int]$Seconds) $script:sleeps.Add($Seconds) }
        Log                = { param([string]$Message) $script:log.Add($Message) }
    }
    $script:ghHandler = {
        param([string[]]$A)
        $option = { param([string]$Name) $A[[Array]::IndexOf($A, $Name) + 1] }
        switch ("$($A[0]) $($A[1])") {
            'run view' { if ($script:runs.ContainsKey($A[2])) { return New-GhResult 0 ($script:runs[$A[2]] | ConvertTo-Json) } ; return New-GhResult 1 '' 'run not found' }
            'run download' {
                $files = $script:artifacts[(& $option '--name')]
                if (-not $files) { return New-GhResult 1 '' 'no artifact' }
                foreach ($relative in $files.Keys) { Write-StandIn (Join-Path (& $option '--dir') $relative) $files[$relative] }
                return New-GhResult 0
            }
            'release view' { if ($script:releases.ContainsKey($A[2])) { return New-GhResult 0 ($script:releases[$A[2]] | ConvertTo-Json -Depth 4) }; return New-GhResult 1 '' 'release not found' }
            'release create' {
                $files = @($A | Where-Object { [IO.Path]::IsPathRooted($_) -and (Test-Path -LiteralPath $_ -PathType Leaf) -and $_ -notlike '*release-notes.md' })
                $script:releases[$A[2]] = [ordered]@{ isDraft = ($A -contains '--draft'); isPrerelease = ($A -contains '--prerelease'); name = (& $option '--title'); tagName = $A[2]
                    assets = @($files | ForEach-Object { @{ name = (Split-Path $_ -Leaf) } }) }
                return New-GhResult 0
            }
            'release upload' { return New-GhResult 0 }
            'release download' { Write-StandIn (Join-Path (& $option '--dir') 'signing-manifest.json') $script:handoffManifest; return New-GhResult 0 }
            'release delete' { $script:releases.Remove($A[2]); return New-GhResult 0 }
            default {
                if ($A[0] -eq 'api' -and $A[1] -match '/git/ref/tags/(.+)$') {
                    $tag = $Matches[1]
                    if ($script:tags.ContainsKey($tag)) { return New-GhResult 0 "commit $($script:tags[$tag])" }
                    return New-GhResult 1 '' 'gh: Not Found (HTTP 404)'
                }
                throw "Unexpected gh call: $($A -join ' ')"
            }
        }
    }
    function Reset-Mocks {
        $script:signatures.Clear(); $script:signCalls.Clear(); $script:ghCalls.Clear(); $script:sleeps.Clear(); $script:log.Clear()
        $script:signFailures = 0; $script:runs = @{}; $script:artifacts = @{}; $script:releases = @{}; $script:tags = @{}
        $script:archiveEntries = @{}; $script:msixPublisher = $config.subject
    }
    $commit = 'a' * 40
    function New-Context([hashtable]$Spec, [string]$Name, [switch]$Publish) {
        @{ Tools = $tools; Config = $config; Spec = $Spec; Repository = 'owner/repo'; OutputRoot = (Join-Path $work $Name); Publish = [bool]$Publish }
    }
    function Get-SignCalls { @($script:signCalls | Where-Object { $_.StartsWith('sign | ') }) }

    # --- Stage 2 (binaries), explicit policy ---------------------------------------------------------
    function New-RustTestSpec {
        $spec = Get-ProductSpec -Product Rust
        $spec.UnsignedArtifact = @{ x64 = 'u-x64'; arm64 = 'u-arm64' }
        $spec.PeExtensions = @('.bin'); $spec.OwnerFiles = @('shell.bin'); $spec.MainExecutable = 'shell.bin'
        $spec
    }
    function Set-RustSourceRun([string]$Workflow = 'Rust Feasibility', [string]$Branch = 'main', [string]$Conclusion = 'success') {
        $script:runs['100'] = [ordered]@{ databaseId = 100; workflowName = $Workflow; status = 'completed'; conclusion = $Conclusion; event = 'push'; headBranch = $Branch; headSha = $commit }
        $script:artifacts['u-x64'] = @{ 'shell.bin' = 'x64 shell'; 'unsigned-binaries.json' = '{}' }
        $script:artifacts['u-arm64'] = @{ 'shell.bin' = 'arm64 shell'; 'unsigned-binaries.json' = '{}' }
    }
    Reset-Mocks; Set-RustSourceRun
    $result = Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-dry') -SourceRunId '100'
    Assert-That (-not $result.Uploaded) 'A dry run must not upload.'
    Assert-That (@($script:ghCalls | Where-Object { $_ -like 'release *' }).Count -eq 0) "A dry run must not touch releases: $($script:ghCalls -join '; ')"
    Assert-That ((Get-SignCalls).Count -eq 1 -and [regex]::Matches(@(Get-SignCalls)[0], 'shell\.bin').Count -eq 2) 'Both shells are signed in one signtool call.'
    Assert-That (@($result.Manifest.files).Count -eq 2 -and @($result.Manifest.kept).Count -eq 0) 'The manifest lists both shells and keeps nothing.'
    $entry = @($result.Manifest.files | Where-Object { $_.architecture -eq 'arm64' })[0]
    Assert-That ($entry.path -ceq 'shell.bin' -and $entry.handoffAsset -ceq 'arm64-002-shell.bin' -and $entry.unsignedSha256 -cmatch '^[0-9a-f]{64}$') 'Manifest entry fields.'
    Assert-That ($result.Manifest.signer.thumbprint -ceq $config.thumbprint -and $result.Manifest.sourceCommit -ceq $commit -and $result.Manifest.handoffTag -ceq 'signing-handoff-rust-100') 'Manifest provenance.'
    $handoffFiles = @(Get-ChildItem -LiteralPath (Join-Path $work 'rust-dry\handoff') -File | ForEach-Object Name | Sort-Object)
    Assert-That (($handoffFiles -join ',') -ceq 'arm64-002-shell.bin,signing-manifest.json,x64-001-shell.bin') "Handoff folder: $($handoffFiles -join ',')"
    Assert-That ([IO.File]::ReadAllText((Join-Path $work 'rust-dry\unsigned\x64\shell.bin')) -ceq 'x64 shell') 'The unsigned download stays untouched.'

    # Resume: the same output folder signs nothing again.
    $script:signCalls.Clear()
    $null = Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-dry') -SourceRunId '100'
    Assert-That ((Get-SignCalls).Count -eq 0) 'A second run must skip files that are already signed and timestamped.'

    # Untimestamped signatures are signed again.
    $key = (Join-Path $work 'rust-dry\signed\x64\shell.bin').ToLowerInvariant()
    $script:signatures[$key] = @{ Thumb = $config.thumbprint; Timestamped = $false }
    $plan = @(Get-SigningPlan -Tools $tools -Config $config -Spec (New-RustTestSpec) -Root (Join-Path $work 'rust-dry\signed\x64'))
    Assert-That ($plan[0].Action -eq 'Sign') 'A signature without a timestamp must be redone.'

    # Publish: a draft handoff release, never a pre-release or a latest release.
    Reset-Mocks; Set-RustSourceRun
    $result = Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-publish' -Publish) -SourceRunId '100'
    $create = @($script:ghCalls | Where-Object { $_ -like 'release create*' })
    Assert-That ($result.Uploaded -and $create.Count -eq 1) 'Publish creates one handoff release.'
    Assert-That ($create[0] -like 'release create signing-handoff-rust-100 *--draft*' -and $create[0] -notlike '*--prerelease*' -and $create[0] -notlike '*--latest*') "Handoff create call: $($create[0])"
    Assert-That ($create[0] -like "*--target $commit*" -and $create[0] -like '*INTERNAL signing handoff*') 'The handoff names its commit and is marked internal.'
    Assert-That ($result.NextCommand -like 'gh workflow run signed-packages.yml *-f product=rust -f source_run_id=100 -f handoff_tag=signing-handoff-rust-100') "Next command: $($result.NextCommand)"
    $script:ghCalls.Clear()
    $null = Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-publish' -Publish) -SourceRunId '100'
    Assert-That (@($script:ghCalls | Where-Object { $_ -like 'release upload signing-handoff-rust-100 *--clobber*' }).Count -eq 1) 'A second publish replaces the draft assets.'
    $script:releases['signing-handoff-rust-100'].isDraft = $false
    Assert-Throws { Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-publish' -Publish) -SourceRunId '100' } 'must stay a draft' 'A published handoff must stop the run.'

    # Source run checks.
    Reset-Mocks; Set-RustSourceRun -Workflow 'CI & Quality Assurance'
    Assert-Throws { Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-wrong-workflow') -SourceRunId '100' } 'expected' 'Another workflow must be refused.'
    Reset-Mocks; Set-RustSourceRun -Branch 'feature'
    Assert-Throws { Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-wrong-branch') -SourceRunId '100' } 'does not match' 'A run that did not build main must be refused.'
    Reset-Mocks; Set-RustSourceRun -Conclusion 'failure'
    Assert-Throws { Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-red') -SourceRunId '100' } 'not green' 'A red run must be refused.'
    Reset-Mocks; Set-RustSourceRun; $script:artifacts['u-x64']['extra.bin'] = 'unexpected'
    Assert-Throws { Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-extra') -SourceRunId '100' } 'Expected exactly' 'An unexpected executable must stop the explicit policy.'
    Reset-Mocks; Set-RustSourceRun; $script:artifacts['u-x64']['shell.bin'] = 'THIRDPARTY shell'
    Assert-Throws { Invoke-BinariesStage -Context (New-Context (New-RustTestSpec) 'rust-thirdparty') -SourceRunId '100' } 'third-party' 'A shell that is already signed by someone else must be refused.'

    # --- Stage 2, "all unsigned" policy (.NET) -------------------------------------------------------
    function New-DotNetTestSpec {
        $spec = Get-ProductSpec -Product DotNet
        $spec.UnsignedArtifact = @{ x64 = 'd-x64'; arm64 = 'd-arm64' }
        $spec.PeExtensions = @('.bin'); $spec.MainExecutable = 'app.bin'
        $spec
    }
    function Set-DotNetSourceRun {
        $script:runs['300'] = [ordered]@{ databaseId = 300; workflowName = 'Package & Release'; status = 'completed'; conclusion = 'success'; event = 'push'; headBranch = 'v1.2.3'; headSha = $commit }
        foreach ($arch in 'x64', 'arm64') {
            $script:artifacts["d-$arch"] = @{ 'app.bin' = "$arch app"; 'sub/lib.bin' = "$arch lib"; 'vendor.bin' = "THIRDPARTY $arch"; 'app.pri' = 'resources' }
        }
    }
    Reset-Mocks; Set-DotNetSourceRun
    $result = Invoke-BinariesStage -Context (New-Context (New-DotNetTestSpec) 'dotnet-dry') -SourceRunId '300'
    $signedPaths = @($result.Manifest.files | Where-Object { $_.architecture -eq 'x64' } | ForEach-Object { $_.path } | Sort-Object)
    Assert-That (($signedPaths -join ',') -ceq 'app.bin,sub/lib.bin') "Unsigned PE files are signed: $($signedPaths -join ',')"
    Assert-That (((@($result.Manifest.kept | ForEach-Object { $_.path }) | Sort-Object -Unique) -join ',') -ceq 'vendor.bin') 'Third-party signed files are kept.'
    Assert-That ([IO.File]::ReadAllText((Join-Path $work 'dotnet-dry\signed\x64\vendor.bin')) -ceq 'THIRDPARTY x64' -and -not $script:signatures.ContainsKey((Join-Path $work 'dotnet-dry\signed\x64\vendor.bin').ToLowerInvariant())) 'A kept file is never re-signed.'
    Reset-Mocks; Set-DotNetSourceRun; $script:artifacts['d-arm64']['bad.bin'] = 'BADSIG'
    Assert-Throws { Invoke-BinariesStage -Context (New-Context (New-DotNetTestSpec) 'dotnet-bad') -SourceRunId '300' } 'invalid signature' 'An invalid signature must stop the run.'

    # --- Retries and batches -------------------------------------------------------------------------
    Reset-Mocks
    $batchFiles = foreach ($i in 1..5) { $path = Join-Path $work "batch\f$i.bin"; Write-StandIn $path "file $i"; $path }
    Invoke-SignFiles -Tools $tools -Config $config -Paths $batchFiles -BatchSize 2
    Assert-That ((Get-SignCalls).Count -eq 3) 'Five files in batches of two need three signtool calls.'
    Reset-Mocks; $script:signFailures = 1
    Invoke-SignFiles -Tools $tools -Config $config -Paths $batchFiles
    Assert-That ((Get-SignCalls).Count -eq 2 -and $script:sleeps.Count -eq 1 -and $script:sleeps[0] -eq 20) 'One timestamp failure is retried after a pause.'
    Reset-Mocks; $script:signFailures = 9
    Assert-Throws { Invoke-SignFiles -Tools $tools -Config $config -Paths $batchFiles } 'SimplySign Desktop is logged in' 'Repeated failures stop with recovery advice.'
    Assert-That ((Get-SignCalls).Count -eq 3 -and $script:sleeps.Count -eq 2) 'At most three attempts with two pauses.'

    # --- Session probe ------------------------------------------------------------------------------
    Reset-Mocks
    Test-SigningSession -Tools $tools -Config $config -WorkDirectory (Join-Path $work 'probe')
    Assert-That (@(Get-ChildItem -LiteralPath (Join-Path $work 'probe') -File).Count -eq 0) 'The probe file is deleted.'
    Reset-Mocks; $script:signFailures = 1
    Assert-Throws { Test-SigningSession -Tools $tools -Config $config -WorkDirectory (Join-Path $work 'probe') } 'log in' 'A failed probe points to SimplySign Desktop.'

    # --- Stage 4 (packages), Rust -------------------------------------------------------------------
    function New-RustPackagesSpec {
        $spec = New-RustTestSpec
        $spec.PackagingArtifact = @{ x64 = 'p-x64'; arm64 = 'p-arm64' }
        $spec.ReleaseFiles = @(
            @{ Pattern = 'portable\*.zipx'; Sign = $null; ContainsMain = $true },
            @{ Pattern = 'portable\*.spdx.json'; Sign = $null; ContainsMain = $false },
            @{ Pattern = 'setup\*.setup.bin'; Sign = 'Pe'; ContainsMain = $false },
            @{ Pattern = 'msix\*.msixx'; Sign = 'Msix'; ContainsMain = $true }
        )
        $spec
    }
    function Set-PackagesScenario([string]$Product = 'rust', [string]$SourceRef = 'main', [string]$ManifestShaOverride) {
        $handoffTag = "signing-handoff-$Product-100"
        $files = foreach ($arch in 'x64', 'arm64') { [ordered]@{ architecture = $arch; path = $(if ($Product -eq 'rust') { 'shell.bin' } else { 'app.bin' }); signedSha256 = "$arch-signed-main" } }
        $script:handoffManifest = [ordered]@{
            schemaVersion = 'marknexia-signing-handoff-v1'; product = $Product; sourceRunId = '100'; sourceRef = $SourceRef; sourceCommit = $commit; handoffTag = $handoffTag
            signer = [ordered]@{ thumbprint = $config.thumbprint; certificateSha256 = $config.certificateSha256; subject = $config.subject }; files = @($files); kept = @()
        } | ConvertTo-Json -Depth 5
        $manifestSha = ([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($script:handoffManifest)) | ForEach-Object { $_.ToString('x2') }) -join ''
        if ($ManifestShaOverride) { $manifestSha = $ManifestShaOverride }
        $script:runs['200'] = [ordered]@{ databaseId = 200; workflowName = 'Signed packages'; status = 'completed'; conclusion = 'success'; event = 'workflow_dispatch'; headBranch = 'main'; headSha = ('b' * 40) }
        $script:releases[$handoffTag] = [ordered]@{ isDraft = $true; isPrerelease = $false; name = "INTERNAL signing handoff (draft, never publish): $Product run 100"; tagName = $handoffTag; assets = @() }
        foreach ($arch in 'x64', 'arm64') {
            $provenance = [ordered]@{ schemaVersion = 'marknexia-signed-packages-provenance-v1'; product = $Product; architecture = $arch; packagingRunId = '200'
                sourceRunId = '100'; sourceCommit = $commit; sourceRef = $SourceRef; handoffTag = $handoffTag; manifestSha256 = $manifestSha } | ConvertTo-Json
            if ($Product -eq 'rust') {
                $script:artifacts["p-$arch"] = @{ "portable/shell-$arch.zipx" = "zip $arch"; "portable/shell-$arch.spdx.json" = '{}'; "setup/setup-$arch.setup.bin" = "setup $arch"
                    "msix/pkg-$arch.msixx" = "msix $arch"; 'setup/SHA256SUMS' = 'stale'; 'packaging-provenance.json' = $provenance }
                $script:archiveEntries["shell-$arch.zipx"] = "$arch-signed-main"; $script:archiveEntries["pkg-$arch.msixx"] = "$arch-signed-main"
            } else {
                $script:artifacts["q-$arch"] = @{ "Marknexia-v1.2.3-win-$arch.zipx" = "zip $arch"; "Marknexia-v1.2.3-win-$arch.msixx" = "msix $arch"
                    "Marknexia-v1.2.3-win-$arch-setup.bin" = "setup $arch"; "Marknexia-v1.2.3-win-$arch-setup.bin.sha256" = 'stale'; 'packaging-provenance.json' = $provenance }
                $script:archiveEntries["Marknexia-v1.2.3-win-$arch.zipx"] = "$arch-signed-main"; $script:archiveEntries["Marknexia-v1.2.3-win-$arch.msixx"] = "$arch-signed-main"
            }
        }
    }
    Reset-Mocks; Set-PackagesScenario
    $result = Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages') -PackagingRunId '200' -Tag 'rust-preview-0.2.0'
    Assert-That (-not $result.Published -and @($script:ghCalls | Where-Object { $_ -like 'release create*' }).Count -eq 0) 'A packages dry run must not publish.'
    $peCall = @(Get-SignCalls | Where-Object { $_ -like '* | /d | *' })
    $msixCall = @(Get-SignCalls | Where-Object { $_ -notlike '* | /d | *' })
    Assert-That ($peCall.Count -eq 1 -and [regex]::Matches($peCall[0], '\.setup\.bin').Count -eq 2) 'Both setup executables are signed as PE files.'
    Assert-That ($msixCall.Count -eq 1 -and [regex]::Matches($msixCall[0], '\.msixx').Count -eq 2) 'Both MSIX packages are signed.'
    Assert-That (@(Get-SignCalls | Where-Object { $_ -match '\.zipx|\.json' }).Count -eq 0) 'ZIPs and SBOMs are not signed.'
    $releaseFolder = Join-Path $work 'rust-packages\release'
    $sums = @([IO.File]::ReadAllLines((Join-Path $releaseFolder 'SHA256SUMS.txt')))
    $releaseNames = @(Get-ChildItem -LiteralPath $releaseFolder -File | ForEach-Object Name | Where-Object { $_ -cne 'SHA256SUMS.txt' })
    Assert-That ($sums.Count -eq 8 -and $releaseNames.Count -eq 8) "SHA256SUMS.txt lists the eight release files ($($sums.Count))."
    foreach ($line in $sums) {
        if ($line -cnotmatch '^([0-9a-f]{64})  (\S+)$') { $failures.Add("Malformed SHA256SUMS line: $line"); continue }
        Assert-That ((Get-FileSha256 (Join-Path $releaseFolder $Matches[2])) -ceq $Matches[1]) "SHA256SUMS mismatch for $($Matches[2])."
    }
    Assert-That (-not (Test-Path -LiteralPath (Join-Path $releaseFolder 'SHA256SUMS'))) 'Stale per-folder checksums are not released.'

    $script:signCalls.Clear(); $script:ghCalls.Clear()
    $result = Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages' -Publish) -PackagingRunId '200' -Tag 'rust-preview-0.2.0'
    $create = @($script:ghCalls | Where-Object { $_ -like 'release create*' })
    Assert-That ((Get-SignCalls).Count -eq 0) 'A resumed packages stage does not sign again.'
    Assert-That ($result.Published -and $create.Count -eq 1) 'Publish creates one release.'
    Assert-That ($create[0] -like 'release create rust-preview-0.2.0 *--prerelease*' -and $create[0] -like '*--latest=false*' -and $create[0] -like "*--target $commit*" -and $create[0] -notlike '*--draft*') "Rust release call: $($create[0])"
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages' -Publish) -PackagingRunId '200' -Tag 'rust-preview-0.2.0' } 'already exists' 'An existing release is never overwritten.'

    Reset-Mocks; Set-PackagesScenario; $script:tags['rust-preview-0.2.0'] = 'f' * 40
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages-tag' -Publish) -PackagingRunId '200' -Tag 'rust-preview-0.2.0' } 'already exists' 'An existing rust-preview tag is refused.'
    Reset-Mocks; Set-PackagesScenario
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages-v') -PackagingRunId '200' -Tag 'v1.0.0' } 'does not match' 'The Rust edition never uses a v* tag.'
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages-notag') -PackagingRunId '200' } 'Give -Tag' 'The Rust edition needs an explicit tag.'
    Reset-Mocks; Set-PackagesScenario; $script:archiveEntries['shell-x64.zipx'] = 'other'
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages-inner') -PackagingRunId '200' -Tag 'rust-preview-0.2.0' } 'does not contain the signed' 'A package without the signed shell is refused.'
    Reset-Mocks; Set-PackagesScenario; $script:msixPublisher = 'CN=Marknexia Rust Preview UNSIGNED, OID.2.25.311729368913984317654407730594956997722=1'
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages-publisher') -PackagingRunId '200' -Tag 'rust-preview-0.2.0' } 'publisher' 'An MSIX with another publisher is refused before signing.'
    Reset-Mocks; Set-PackagesScenario -ManifestShaOverride ('0' * 64)
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages-manifest') -PackagingRunId '200' -Tag 'rust-preview-0.2.0' } 'differs' 'A handoff manifest that changed after packaging is refused.'
    Reset-Mocks; Set-PackagesScenario; $script:runs['200'].workflowName = 'Rust Feasibility'
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-RustPackagesSpec) 'rust-packages-run') -PackagingRunId '200' -Tag 'rust-preview-0.2.0' } 'expected' 'Packages must come from the Signed packages workflow.'

    # --- Stage 4, .NET ------------------------------------------------------------------------------
    function New-DotNetPackagesSpec {
        $spec = New-DotNetTestSpec
        $spec.PackagingArtifact = @{ x64 = 'q-x64'; arm64 = 'q-arm64' }
        $spec.ReleaseFiles = @(
            @{ Pattern = 'Marknexia-v*-win-*.zipx'; Sign = $null; ContainsMain = $true },
            @{ Pattern = 'Marknexia-v*-win-*.msixx'; Sign = 'Msix'; ContainsMain = $true },
            @{ Pattern = 'Marknexia-v*-win-*-setup.bin'; Sign = 'Pe'; ContainsMain = $false }
        )
        $spec
    }
    Reset-Mocks; Set-PackagesScenario -Product dotnet -SourceRef 'v1.2.3'
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-DotNetPackagesSpec) 'dotnet-packages-wrongtag') -PackagingRunId '200' -Tag 'v1.2.4' } 'tag that built' 'The .NET release must use the tag that built it.'
    $result = Invoke-PackagesStage -Context (New-Context (New-DotNetPackagesSpec) 'dotnet-packages') -PackagingRunId '200'
    Assert-That ($result.Tag -ceq 'v1.2.3') 'The .NET tag defaults to the source tag.'
    $dotnetRelease = Join-Path $work 'dotnet-packages\release'
    foreach ($name in 'Marknexia-v1.2.3-win-x64.zipx', 'Marknexia-v1.2.3-win-arm64.msixx', 'Marknexia-v1.2.3-win-x64-setup.bin') {
        $sidecar = [IO.File]::ReadAllText((Join-Path $dotnetRelease "$name.sha256"))
        Assert-That ($sidecar -ceq "$(Get-FileSha256 (Join-Path $dotnetRelease $name))  $name`n") "Sidecar checksum for $name."
        Assert-That ($sidecar -cmatch '\b[0-9a-fA-F]{64}\b') 'The .NET updater must find the hash in the sidecar.'
    }
    Assert-That (@(Get-ChildItem -LiteralPath $dotnetRelease -File).Count -eq 12) 'Six .NET packages and six checksums.'
    $script:ghCalls.Clear()
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-DotNetPackagesSpec) 'dotnet-packages' -Publish) -PackagingRunId '200' } 'must exist' 'The v* tag must exist before the .NET release is published.'
    $script:tags['v1.2.3'] = 'e' * 40
    Assert-Throws { Invoke-PackagesStage -Context (New-Context (New-DotNetPackagesSpec) 'dotnet-packages' -Publish) -PackagingRunId '200' } 'must exist and point' 'The v* tag must point to the signed commit.'
    $script:tags['v1.2.3'] = $commit; $script:ghCalls.Clear()
    $null = Invoke-PackagesStage -Context (New-Context (New-DotNetPackagesSpec) 'dotnet-packages' -Publish) -PackagingRunId '200'
    $create = @($script:ghCalls | Where-Object { $_ -like 'release create*' })
    Assert-That ($create.Count -eq 1 -and $create[0] -like 'release create v1.2.3 *--verify-tag*--latest*' -and $create[0] -notlike '*--prerelease*' -and $create[0] -notlike '*--target*') ".NET release call: $($create[0])"

    # --- Cleanup ------------------------------------------------------------------------------------
    Reset-Mocks; Set-PackagesScenario
    $script:ghCalls.Clear()
    Assert-That (-not (Invoke-CleanupStage -Context (New-Context (New-RustTestSpec) 'cleanup') -SourceRunId '100')) 'A cleanup dry run removes nothing.'
    Assert-That (@($script:ghCalls | Where-Object { $_ -like 'release delete*' }).Count -eq 0) 'A cleanup dry run calls no delete.'
    Assert-That (Invoke-CleanupStage -Context (New-Context (New-RustTestSpec) 'cleanup' -Publish) -SourceRunId '100') 'Cleanup removes the draft.'
    $delete = @($script:ghCalls | Where-Object { $_ -like 'release delete*' })
    Assert-That ($delete.Count -eq 1 -and $delete[0] -ceq 'release delete signing-handoff-rust-100 --repo owner/repo --yes') "Cleanup call: $($delete[0])"
    Reset-Mocks; $script:releases['signing-handoff-rust-100'] = [ordered]@{ isDraft = $false; isPrerelease = $false; name = 'Something else'; tagName = 'signing-handoff-rust-100'; assets = @() }
    Assert-Throws { Invoke-CleanupStage -Context (New-Context (New-RustTestSpec) 'cleanup' -Publish) -SourceRunId '100' } 'not a signing handoff draft' 'Cleanup never removes a published release.'

    # --- Source contracts ---------------------------------------------------------------------------
    $scripts = @('scripts\signing\MarknexiaSigning.psm1', 'scripts\Invoke-ReleaseSigning.ps1', 'scripts\Test-SignedHandoff.ps1')
    foreach ($relative in $scripts) {
        $path = Join-Path $RepositoryRoot $relative
        $tokens = $null; $parseErrors = $null
        [void][Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$parseErrors)
        Assert-That ($parseErrors.Count -eq 0) "$relative does not parse."
        $text = [IO.File]::ReadAllText($path)
        foreach ($forbidden in 'New-SelfSignedCertificate', 'Import-PfxCertificate', 'Export-PfxCertificate', 'Import-Certificate', 'ConvertTo-SecureString', 'Read-Host', 'GH_TOKEN', 'auth token') {
            Assert-That (-not $text.Contains($forbidden)) "$relative must not use $forbidden."
        }
    }
    $entry = [IO.File]::ReadAllText((Join-Path $RepositoryRoot 'scripts\Invoke-ReleaseSigning.ps1'))
    $argumentCheck = $entry.IndexOf("-Tag is used only by the Packages stage", [StringComparison]::Ordinal)
    $probe = $entry.IndexOf('Test-SigningSession -Tools', [StringComparison]::Ordinal)
    Assert-That ($argumentCheck -ge 0 -and $probe -gt $argumentCheck) 'Arguments are checked before the first signature (the session probe).'
    Assert-That ($entry.Contains('Publish = [bool]$Publish')) 'Publishing needs the explicit -Publish switch.'
    $release = [IO.File]::ReadAllText((Join-Path $RepositoryRoot '.github\workflows\release.yml'))
    Assert-That (-not $release.Contains('action-gh-release') -and -not $release.Contains('gh release')) 'release.yml must not publish unsigned files.'
    Assert-That ($release.Contains('marknexia-dotnet-UNSIGNED-app-x64') -and $release.Contains('marknexia-dotnet-UNSIGNED-app-arm64') -and $release -match 'contents:\s*read') 'release.yml uploads the unsigned app output with read-only contents.'
    $packages = [IO.File]::ReadAllText((Join-Path $RepositoryRoot '.github\workflows\signed-packages.yml'))
    foreach ($forbidden in 'signtool sign', 'secrets.', 'gh release create', 'gh release upload', 'gh release delete', 'pull_request', 'push:') {
        Assert-That (-not $packages.Contains($forbidden)) "signed-packages.yml must not contain '$forbidden'."
    }
    Assert-That ($packages.Contains('Test-SignedHandoff.ps1') -and $packages.Contains('-SignedShellPath') -and $packages.Contains('-Signed') -and $packages.Contains('-PublisherId $signing.subject')) 'signed-packages.yml verifies the handoff and builds signed-mode packages.'
    $feasibility = [IO.File]::ReadAllText((Join-Path $RepositoryRoot '.github\workflows\rust-feasibility.yml'))
    Assert-That ($feasibility.Contains('marknexia-rust-UNSIGNED-binaries-${{ matrix.target }}') -and $feasibility.Contains('tests/signing/release_signing.tests.ps1')) 'Rust CI uploads the unsigned shell and runs these tests.'
    foreach ($name in @($rustSpec.UnsignedArtifact.Values) + @($rustSpec.PackagingArtifact.Values)) {
        $pattern = $name -replace '(x86_64|aarch64)-pc-windows-msvc$', '${{ matrix.target }}'
        Assert-That ($feasibility.Contains($pattern) -or $packages.Contains($pattern)) "Artifact $name is produced by a workflow."
    }
    foreach ($name in @($dotnetSpec.UnsignedArtifact.Values)) { Assert-That ($release.Contains($name)) "Artifact $name is produced by release.yml." }
    foreach ($name in @($dotnetSpec.PackagingArtifact.Values)) { Assert-That ($packages.Contains($name)) "Artifact $name is produced by signed-packages.yml." }
    Assert-That ($packages -match "(?m)^name: $([regex]::Escape($rustSpec.PackagingWorkflow))\s*$") 'The packaging workflow name matches the product spec.'

    # The Rust MSIX template: unsigned rendering is unchanged; signed rendering uses the certificate subject.
    $template = [IO.File]::ReadAllText((Join-Path $RepositoryRoot 'packaging\rust-msix\AppxManifest.xml'))
    $render = { param([string]$Publisher) [xml]($template.Replace('@VERSION@', '0.1.0.0').Replace('@ARCHITECTURE@', 'x64').Replace('@PUBLISHER@', [Security.SecurityElement]::Escape($Publisher)).Replace('@PUBLISHER_DISPLAY_NAME@', 'x').Replace('@DESCRIPTION@', 'x').Replace('@TILE_DESCRIPTION@', 'x')) }
    $signedManifest = & $render $config.subject
    Assert-That ($signedManifest.Package.Identity.Publisher -ceq $config.subject) 'The signed Rust MSIX publisher is the certificate subject.'
    Assert-That ($template -cnotmatch '@(?!VERSION@|ARCHITECTURE@|PUBLISHER@|PUBLISHER_DISPLAY_NAME@|DESCRIPTION@|TILE_DESCRIPTION@)[A-Z_]+@') 'The template has only known tokens.'
    $installers = [IO.File]::ReadAllText((Join-Path $RepositoryRoot 'scripts\New-RustInstallerPackages.ps1'))
    Assert-That ($installers.Contains("'CN=Marknexia Rust Preview UNSIGNED, OID.2.25.311729368913984317654407730594956997722=1'")) 'The unsigned MSIX keeps its unsigned publisher.'
    $store = [IO.File]::ReadAllText((Join-Path $RepositoryRoot 'scripts\prepare-store-package.ps1'))
    Assert-That ($store.Contains('[string]$PublisherId = "CN=4BC6AFD6-49C8-46B2-A096-9AF3F2B78CB8"')) 'The Microsoft Store identity stays the default of prepare-store-package.ps1.'
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures.Count -gt 0) { throw "Release signing tests failed:`n$($failures -join "`n")" }
Write-Host 'Release signing tests passed: configuration, signtool arguments, PE content digest, certificate checks, binaries/packages/cleanup stages (dry run, publish, resume, retries, refusals) and source contracts.'
